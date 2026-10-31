#!/usr/bin/env bash
# The temper pin: every rev of arni-labs/temper this repository builds against (each Cargo.toml line that names
# arni-labs/temper.git with a rev, and each Cargo.lock that names it), moved together in one pull request.
#
#   temper_pin.sh pin      Moves every rev to COMMIT in a pull request on bot/temper-pin-<12>, merges it once its
#                          checks are green, and writes the main commit that pins COMMIT as `commit` to GITHUB_OUTPUT.
#                          When every rev on main already carries COMMIT it opens nothing: main pins it.
#   temper_pin.sh image    Waits for docker.yml's push run on COMMIT (a commit here) and writes its tag, sha-<7>, as `tag`.
#   temper_pin.sh carried  Passes when every temper rev at IMAGE_COMMIT (a commit here) carries COMMIT (a temper commit).
#   temper_pin.sh revert   Reverts the pin pull request of a change that was rolled back: the merged pull request on
#                          bot/temper-pin-<12> for a kernel change's COMMIT, or CHANGE_PR itself when it is a pull
#                          request here that moved a temper rev. Already reverted, or nothing to revert, is a pass.
#
# Environment: COMMIT; CHANGE_PR, the change's pull request address, which a failure names; GH_TOKEN, the GitHub App's
# installation token for this repository for pin and revert (a push, pull request or merge made with the workflow's own
# token starts no workflow here, so the pin's checks and its image would never run), the workflow's token otherwise;
# READ_TOKEN, the workflow's token, which reads arni-labs/temper (public); APP_SLUG, the App's name, its commits' author.
set -euo pipefail

REPO="${GITHUB_REPOSITORY:?}"
TEMPER=arni-labs/temper
REV='arni-labs/temper\.git.*rev = "[0-9a-f]{40}"'
# A pull request's checks and a docker.yml run each take minutes, so they are read every 30 seconds.
POLL=30
# A push starts its docker.yml run within a minute; none 10 minutes after the merge means it never started.
NO_RUN_SECONDS=600

out() { echo "$1=$2" >> "${GITHUB_OUTPUT:-/dev/null}"; }
fail() {
  echo "::error title=temper pin::$1"
  exit 1
}

# ---------- the revs ----------

# The manifests that pin temper, the locks that name it, and the distinct revs (in the work tree, or in tree $1).
pin_manifests() { git grep -l -E "$REV" -- '*Cargo.toml' || true; }
pin_locks() { git grep -l 'github.com/arni-labs/temper' -- '*Cargo.lock' || true; }
revs() { { git grep -h -o -E "$REV" ${1:+"$1"} -- '*Cargo.toml' || true; } | grep -o -E '[0-9a-f]{40}' | sort -u; }
lock_revs() { { git grep -h -o -E 'github\.com/arni-labs/temper\.git\?rev=[0-9a-f]{40}#[0-9a-f]{40}' -- '*Cargo.lock' || true; } | sed 's/.*#//' | sort -u; }

# Whether $2 carries $1 on arni-labs/temper: the same commit, or one after it.
carries() {
  local status
  status="$(GH_TOKEN="${READ_TOKEN:?}" gh api "repos/$TEMPER/compare/$1...$2" --jq .status)" || return 1
  [ "$status" = identical ] || [ "$status" = ahead ]
}

# Whether every temper rev in tree $2 (the work tree when empty) carries $1. A tree that pins no rev carries nothing.
all_carry() {
  local rev all
  all="$(revs "${2:-}")"
  [ -n "$all" ] || return 1
  for rev in $all; do carries "$1" "$rev" || return 1; done
}

# Every rev line, then every lock that names temper, moved to $1. `cargo update --workspace` re-locks what the moved
# revs changed (a new kernel's new dependencies included) and leaves every other locked version as it is.
move_revs() {
  local commit="$1" f lock
  for f in $(pin_manifests); do
    COMMIT="$commit" perl -pi -e 's/rev = "[0-9a-f]{40}"/rev = "$ENV{COMMIT}"/ if m{arni-labs/temper\.git}' "$f"
  done
  for lock in $(pin_locks); do
    echo "cargo update in $(dirname "$lock")"
    (cd "$(dirname "$lock")" && cargo update --workspace)
  done
  [ "$(revs)" = "$commit" ] || fail "after the move the temper revs are $(revs | tr '\n' ' '), not only $commit"
  [ "$(lock_revs)" = "$commit" ] || fail "after cargo update the locks name temper $(lock_revs | tr '\n' ' '), not only $commit"
}

# ---------- a pull request ----------

# Its state, merge commit, head, merge state and the checks on its head, as JSON.
pull() {
  gh api graphql -F owner="${REPO%%/*}" -F name="${REPO#*/}" -F number="$1" -f query='
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      url state headRefOid mergeStateStatus
      mergeCommit { oid }
      commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { name conclusion detailsUrl isRequired(pullRequestNumber: $number) checkSuite { app { slug } } }
        ... on StatusContext { context state targetUrl isRequired(pullRequestNumber: $number) }
      } } } } } }
    }
  }
}' --jq .data.repository.pullRequest
}

# The red checks on its head, one "<name> <link>" per line: a required check that failed, or any of this repository's
# own workflows that failed (only `checks` is required, and a pin that breaks the rest of CI must not land either).
red_checks() {
  jq -r '.commits.nodes[0].commit.statusCheckRollup.contexts.nodes[]? |
    select((.__typename == "CheckRun" and (.isRequired or .checkSuite.app.slug == "github-actions")
             and ((.conclusion // "") | IN("FAILURE", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE")))
        or (.__typename == "StatusContext" and .isRequired and (.state | IN("FAILURE", "ERROR"))))
    | "\(.name // .context) \(.detailsUrl // .targetUrl // "")"'
}

# The newest main, fetched; prints its commit when every temper rev on it carries $1.
main_pins() {
  git fetch -q --depth=1 origin main
  all_carry "$1" FETCH_HEAD && git rev-parse FETCH_HEAD
}

# The newest run of each workflow on commit $1 for its pull request (a pull request closed and reopened runs them
# again on the same head): "pending" until one has started and every one has finished, "green" when every one passed,
# otherwise the first that did not, as "<name> <conclusion>: <link>". A run that ended in any other way (failed,
# cancelled, timed out) is red: its checks never passed on this head.
ci_runs() {
  gh api "repos/$REPO/actions/runs?head_sha=$1&event=pull_request&per_page=100" --jq '
    (.workflow_runs | group_by(.workflow_id) | map(max_by(.id))) as $runs
    | ($runs | map(select(.conclusion | IN("success", "skipped", "neutral") | not))) as $red
    | if ($runs | length) == 0 or ($runs | any(.status != "completed")) then "pending"
      elif ($red | length) == 0 then "green"
      else $red[0] | "\(.name) \(.conclusion): \(.html_url)" end'
}

# Lands pull request $1: it is merged, with the App's token so the merge's push starts docker.yml, once every workflow
# run on its head has passed and GitHub calls it mergeable. GitHub's own auto-merge is not used: it
# waits only for the required checks, and only `checks` is required, so a pin that fails cargo test would land. The
# pull request is brought up to date with main while it is behind (main requires that). Writes the merge commit as
# `commit`. $2 is "pin" for a pin, whose failure goes to its change's maker, or "revert".
land() {
  local n="$1" kind="$2" state url tip red merge_state at
  while :; do
    state="$(pull "$n")"
    url="$(jq -r .url <<<"$state")"
    tip="$(jq -r .headRefOid <<<"$state")"
    merge_state="$(jq -r .mergeStateStatus <<<"$state")"
    case "$(jq -r .state <<<"$state")" in
      MERGED)
        out commit "$(jq -r .mergeCommit.oid <<<"$state")"
        echo "$url merged as $(jq -r .mergeCommit.oid <<<"$state")"
        return
        ;;
      CLOSED) fail "$url was closed without merging" ;;
    esac
    # Main moved. When a later pin landed there first it carries this one: this pull request is no longer needed.
    if [ "$kind" = pin ] && { [ "$merge_state" = BEHIND ] || [ "$merge_state" = DIRTY ]; } && at="$(main_pins "$COMMIT")"; then
      gh pr close "$n" -R "$REPO" --comment "Main's temper revs carry ${COMMIT:0:12} since ${at:0:12}." || true
      out commit "$at"
      echo "main $at already pins ${COMMIT:0:12}: closed $url"
      return
    fi
    red="$(red_checks <<<"$state")"
    if [ -n "$red" ]; then
      broken "$kind" "$url" "is red" "At ${tip:0:12}: $(head -n 1 <<<"$red")$([ "$(wc -l <<<"$red")" -gt 1 ] && echo " and $(($(wc -l <<<"$red") - 1)) more")."
    fi
    case "$merge_state" in
      DIRTY) broken "$kind" "$url" "conflicts with main" "Merge main into its branch and resolve it." ;;
      BEHIND)
        echo "$url is behind main: updating its branch"
        gh api -X PUT "repos/$REPO/pulls/$n/update-branch" -f expected_head_sha="$tip" >/dev/null || true
        ;;
      CLEAN | UNSTABLE | HAS_HOOKS)
        # The runs are read after the checks above, so a run that failed since they were read stops the merge too. A
        # push since then cancels the runs on the old head, which then says nothing about the new one.
        red="$(ci_runs "$tip")"
        case "$red" in
          pending) ;;
          green) gh pr merge "$n" -R "$REPO" --squash --match-head-commit "$tip" || true ;;
          *) [ "$(pull "$n" | jq -r .headRefOid)" != "$tip" ] || broken "$kind" "$url" "is red" "At ${tip:0:12}: $red." ;;
        esac
        ;;
    esac
    sleep "$POLL"
  done
}

# Pull request $2 cannot land as it is: $3 says why, $4 what failed. For a pin the outcome names the pull request,
# and the message, which a reader may cut short, starts with what the change's maker does: fix that branch, never the
# change that merged, and end with the change's own pull request.
broken() {
  local message="$2 $3. $4"
  if [ "$1" = pin ]; then
    message="pin PR $2 $3: fix its branch $BRANCH, not the merged change, then end your answer with ${CHANGE_PR:-the pull request of the merged change}. $4"
    echo "::notice title=outcome::$(jq -cn --arg fix "$2" --arg reason "$message" '{result: "failed", fix: $fix, reason: $reason}')"
  fi
  echo "::error title=temper pin::$message"
  exit 1
}

# The pull requests on branch $1, as JSON.
on_branch() { gh pr list -R "$REPO" --head "$1" --state all --json number,state,url; }

# Commits here are the App's own.
as_app() {
  local id
  id="$(gh api "users/${APP_SLUG:?}%5Bbot%5D" --jq .id)"
  git config user.name "${APP_SLUG}[bot]"
  git config user.email "${id}+${APP_SLUG}[bot]@users.noreply.github.com"
}

# ---------- pin ----------

pin() {
  local commit="${COMMIT:-}" main prs n url body
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail "commit must be a whole arni-labs/temper commit, not \"$commit\""
  main="$(GH_TOKEN="${READ_TOKEN:?}" gh api "repos/$TEMPER/commits/main" --jq .sha)"
  carries "$commit" "$main" || fail "$commit is not on arni-labs/temper main (main is $main)"
  [ -n "$(revs)" ] || fail "no Cargo.toml here pins arni-labs/temper by rev"
  BRANCH="bot/temper-pin-${commit:0:12}"
  prs="$(on_branch "$BRANCH")"

  if all_carry "$commit"; then
    # Nothing to pin. An open pull request for it is no longer needed, and merged later it would move a rev back.
    for n in $(jq -r '.[] | select(.state == "OPEN") | .number' <<<"$prs"); do
      gh pr close "$n" -R "$REPO" --comment "Every temper rev on main already carries ${commit:0:12}." || true
    done
    out commit "$(git rev-parse HEAD)"
    echo "every temper rev here already carries ${commit:0:12}: main's $(git rev-parse --short=12 HEAD) pins it"
    return
  fi
  echo "temper revs here: $(revs | tr '\n' ' ')"

  n="$(jq -r '[.[] | select(.state == "OPEN")][0].number // empty' <<<"$prs")"
  if [ -n "$n" ]; then
    echo "reusing open pull request #$n on $BRANCH, as its branch stands"
  else
    url="$(jq -r '[.[] | select(.state == "CLOSED")][0].url // empty' <<<"$prs")"
    [ -z "$url" ] || fail "$url pinned ${commit:0:12} and was closed without merging; it is not opened again"
    url="$(jq -r '[.[] | select(.state == "MERGED")][0].url // empty' <<<"$prs")"
    [ -z "$url" ] || fail "$url pinned ${commit:0:12} and merged, but main's revs no longer carry it: it was reverted since"
    if git ls-remote --exit-code --heads origin "$BRANCH" >/dev/null; then
      # A run before this one pushed the branch and stopped before its pull request: the branch is used as it stands.
      echo "$BRANCH exists with no pull request: opening one from it"
    else
      move_revs "$commit"
      as_app
      git switch -q -c "$BRANCH"
      git add -A -- '*Cargo.toml' '*Cargo.lock'
      git commit -q -m "deps: pin Temper to ${commit:0:12}" -m "Moves every arni-labs/temper rev to $commit.${CHANGE_PR:+ For $CHANGE_PR.}"
      git push -q origin "HEAD:refs/heads/$BRANCH"
    fi
    body="$RUNNER_TEMP/pin-body.md"
    {
      echo "Moves every arni-labs/temper rev here ($(pin_manifests | wc -l | tr -d ' ') manifests, $(pin_locks | wc -l | tr -d ' ') locks) to [\`${commit:0:12}\`](https://github.com/$TEMPER/commit/$commit)${CHANGE_PR:+, for $CHANGE_PR}."
      echo
      echo "Opened by temper-pin-bump.yml, the staging step of a kernel change. A version update takes no review: the run that opened it merges it once its checks are green, then waits for its image and deploys that to staging. A red check goes to the change's maker, who fixes it on this branch."
    } > "$body"
    gh pr create -R "$REPO" --base main --head "$BRANCH" --title "deps: pin Temper to ${commit:0:12}" --body-file "$body"
    n="$(jq -r '[.[] | select(.state == "OPEN")][0].number' <<<"$(on_branch "$BRANCH")")"
  fi
  land "$n" pin
}

# ---------- image ----------

image() {
  local commit="${COMMIT:-}" since run
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail "commit must be a whole commit here, not \"$commit\""
  since="$(date +%s)"
  while :; do
    run="$(gh api "repos/$REPO/actions/workflows/docker.yml/runs?head_sha=$commit&event=push&per_page=1" --jq '.workflow_runs[0] // empty')"
    if [ -z "$run" ]; then
      [ $(($(date +%s) - since)) -lt "$NO_RUN_SECONDS" ] || fail "no docker.yml push run on $commit after $((NO_RUN_SECONDS / 60)) minutes"
    elif [ "$(jq -r .status <<<"$run")" = completed ]; then
      [ "$(jq -r .conclusion <<<"$run")" = success ] || fail "docker.yml's run on ${commit:0:12} ended $(jq -r .conclusion <<<"$run"): $(jq -r .html_url <<<"$run")"
      out tag "sha-${commit:0:7}"
      echo "ghcr.io/$REPO:sha-${commit:0:7} built by $(jq -r .html_url <<<"$run")"
      return
    fi
    sleep "$POLL"
  done
}

# ---------- carried ----------

carried() {
  [[ "${COMMIT:-}" =~ ^[0-9a-f]{40}$ ]] || fail "commit must be a whole arni-labs/temper commit, not \"${COMMIT:-}\""
  [[ "${IMAGE_COMMIT:-}" =~ ^[0-9a-f]{40}$ ]] || fail "the image's commit must be a whole commit here, not \"${IMAGE_COMMIT:-}\""
  git fetch -q --depth=1 origin "$IMAGE_COMMIT"
  all_carry "$COMMIT" FETCH_HEAD || fail "${IMAGE_COMMIT:0:12} is neither commit $COMMIT nor pins it: its temper revs are $(revs FETCH_HEAD | tr '\n' ' ')"
  echo "every temper rev at ${IMAGE_COMMIT:0:12} carries ${COMMIT:0:12}"
}

# ---------- revert ----------

revert() {
  local commit="${COMMIT:-}" change="${CHANGE_PR:-}" prs n state url sha back url_back n_back mainline
  if [ -z "$commit$change" ]; then
    echo "the roll-back names no change: no pin to revert"
    return
  elif [[ "$change" =~ ^https://github\.com/${REPO}/pull/([0-9]+)/?$ ]]; then
    # A change here: its own pull request is its pin pull request when it moved a temper rev.
    n="${BASH_REMATCH[1]}"
  else
    [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail "commit must be a whole arni-labs/temper commit, not \"$commit\""
    prs="$(on_branch "bot/temper-pin-${commit:0:12}")"
    n="$(jq -r '[.[] | select(.state == "MERGED")][0].number // empty' <<<"$prs")"
    if [ -z "$n" ]; then
      # Rolled back before its pin landed: an open pin pull request is closed, so it does not land after.
      for n in $(jq -r '.[] | select(.state == "OPEN") | .number' <<<"$prs"); do
        gh pr close "$n" -R "$REPO" --comment "Its kernel change was rolled back before this landed." || true
      done
      echo "no pin of ${commit:0:12} merged here: nothing to revert"
      return
    fi
  fi
  state="$(pull "$n")"
  url="$(jq -r .url <<<"$state")"
  [ "$(jq -r .state <<<"$state")" = MERGED ] || { echo "$url is not merged: nothing to revert"; return; }
  sha="$(jq -r .mergeCommit.oid <<<"$state")"
  git fetch -q --depth=2 origin "$sha"
  if ! git diff "$sha^1" "$sha" -- '*Cargo.toml' | grep -q -E '^[-+].*arni-labs/temper\.git'; then
    echo "$url moved no temper rev: nothing to revert"
    return
  fi

  back="bot/temper-unpin-$n"
  prs="$(on_branch "$back")"
  url_back="$(jq -r '[.[] | select(.state == "MERGED")][0].url // empty' <<<"$prs")"
  [ -z "$url_back" ] || { echo "$url was reverted already by $url_back"; return; }
  n_back="$(jq -r '[.[] | select(.state == "OPEN")][0].number // empty' <<<"$prs")"
  if [ -z "$n_back" ]; then
    url_back="$(jq -r '[.[] | select(.state == "CLOSED")][0].url // empty' <<<"$prs")"
    [ -z "$url_back" ] || fail "the revert of $url, $url_back, was closed without merging"
    if git ls-remote --exit-code --heads origin "$back" >/dev/null; then
      # A run before this one pushed the revert and stopped before its pull request: the branch is used as it stands.
      echo "$back exists with no pull request: opening one from it"
    else
      # A squash merge reverts as it is; a merge commit reverts against main, its first parent.
      mainline=()
      [ "$(git cat-file -p "$sha" | grep -c '^parent ')" -eq 1 ] || mainline=(-m 1)
      if ! git revert --no-commit "${mainline[@]}" "$sha"; then
        fail "reverting $url conflicts with main (main's temper revs are $(revs | tr '\n' ' ')): $(git diff --name-only --diff-filter=U | tr '\n' ' ')"
      fi
      if git diff --cached --quiet; then
        echo "main no longer carries $url: nothing to revert"
        return
      fi
      as_app
      git switch -q -c "$back"
      git commit -q -m "Revert the temper pin of #$n" -m "Its change was rolled back. Reverts $sha."
      git push -q origin "HEAD:refs/heads/$back"
    fi
    gh pr create -R "$REPO" --base main --head "$back" --title "Revert the temper pin of #$n" \
      --body "Its change was rolled back, and its image with it. This takes the pin it moved off main, so the next image does not carry it again. Reverts $url ($sha)."
    n_back="$(jq -r '[.[] | select(.state == "OPEN")][0].number' <<<"$(on_branch "$back")")"
  fi
  land "$n_back" revert
}

case "${1:-}" in
  pin | image | carried | revert) "$1" ;;
  *) fail "usage: temper_pin.sh pin|image|carried|revert" ;;
esac
