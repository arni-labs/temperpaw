//! seed_world — spawns the skeleton builders for a world (ADR-002).
//!
//! On World.Seed: spawn an open researcher for semantic exploration, or
//! a determined-fact surveyor for legacy corridor sampling. The surveyor self-reports World.SeedComplete. Bookmaker
//! (market-priced marginals) is disabled — it wasted eval time searching
//! prediction markets the user did not ask for.
//!
//! Hindcast worlds (hindcast_mode = "true") get web tools stripped at
//! Configure time: evidence comes only from the frozen corpus.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const SURVEYOR_TOOLS: &str =
    "temper_get,temper_list,temper_create,temper_action,temper_read,temper_write";
const WEB_TOOLS: &str = ",temper_web_search,temper_web_fetch";

// Keep native diagnostic fields only; never serialize response headers or the request.
fn session_create_error(name: &str, status: u16, body: &str) -> String {
    let prefix = format!("create Session for {name} failed (HTTP {status})");
    if body.len() > 64 * 1024 {
        return format!("{prefix}: response detail exceeds 64 KiB bound");
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return prefix;
    };
    let mut details = Vec::new();
    for path in [
        "/error/code",
        "/error/message",
        "/error",
        "/message",
        "/denial_reason",
        "/decision_id",
        "/pending_decision_id",
        "/error/decision_id",
        "/error/pending_decision_id",
    ] {
        if let Some(text) = value.pointer(path).and_then(Value::as_str) {
            let bounded: String = text.chars().filter(|c| !c.is_control()).take(512).collect();
            if !bounded.is_empty() {
                details.push(format!("{}={bounded}", path.trim_start_matches('/')));
            }
        }
    }
    if details.is_empty() {
        prefix
    } else {
        format!("{prefix}: {}", details.join("; "))
    }
}

fn tools_enabled(hindcast: bool) -> String {
    if hindcast {
        SURVEYOR_TOOLS.to_string()
    } else {
        format!("{SURVEYOR_TOOLS}{WEB_TOOLS}")
    }
}

/// Truncate inlined corpus content at a char boundary; the skeleton must
/// never be grounded in silently-missing text, so the truncation is loud.
fn inline_corpus(raw: &str) -> String {
    const CAP: usize = 30_000;
    if raw.len() <= CAP {
        return raw.to_string();
    }
    let mut end = CAP;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[TRUNCATED AT 30KB — record only facts whose sources survived the cut]",
        &raw[..end]
    )
}

/// Fetch the corpus content for prompt inlining. A configured corpus that
/// cannot be read is a hard error — a world grounded in a corpus nobody can
/// see is not a world.
fn fetch_corpus(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    corpus_file_id: &str,
) -> Result<String, String> {
    if corpus_file_id.is_empty() {
        return Ok(String::new());
    }
    let url = format!("{api}/tdata/Files('{corpus_file_id}')/$value");
    let resp = ctx.http_call("GET", &url, headers, "")?;
    if resp.status < 200 || resp.status >= 300 {
        return Err(format!(
            "seed_world: could not fetch corpus {corpus_file_id} (HTTP {})",
            resp.status
        ));
    }
    Ok(inline_corpus(&resp.body))
}

/// The surveyor's working contract. Single source of truth for the action
/// and parameter names it must use — tested below, asserted nowhere else.
fn surveyor_prompt(
    world_id: &str,
    agent_id: &str,
    name: &str,
    domain: &str,
    description: &str,
    target_date: &str,
    corpus_inline: &str,
    hindcast: bool,
) -> String {
    let corpus_line = if corpus_inline.is_empty() {
        "No corpus file was provided.".to_string()
    } else {
        // Inlined, not temper.read: session file reads resolve by path inside
        // the session's workspace, and a harness-uploaded corpus has neither —
        // three live surveyor sessions died thrashing on this before the
        // gate's inline pattern (wall 9) was applied here too.
        format!(
            "The world corpus — the domain documents this world is grounded in — is \
             inlined below between BEGIN CORPUS and END CORPUS.\n\n\
             --- BEGIN CORPUS ---\n{corpus_inline}\n--- END CORPUS ---"
        )
    };
    let research_line = if hindcast {
        "This is a HINDCAST world: you have NO web access by design. Use only the corpus. \
         Never state anything dated after the world's vantage."
            .to_string()
    } else {
        "Use temper.web_search / temper.web_fetch to verify what is already determined.".to_string()
    };
    format!(
        "You are the Surveyor for world {world_id} (\"{name}\", domain: {domain}).\n\
         {description}\n\n\
         Your job is the skeleton: record what is ALREADY DETERMINED about this domain \
         between today and {target_date}. You are forbidden from predicting. Determined \
         means: demographics already alive, infrastructure already funded or under \
         construction, dated commitments (elections, expirations, scheduled releases, \
         contract cliffs), regulations already enacted with future effect. If a claim \
         needs a probability, it is not yours to record.\n\n\
         {corpus_line}\n{research_line}\n\n\
         EXECUTION RULES (Monty REPL — you only have the execute tool; each call is one Python \
         script that runs to completion before the next turn):\n\
         - NEVER put the whole job in one execute() call. A monolithic script on turn 1 is a \
         known failure mode: the session can mark complete without running it.\n\
         - Use MANY SMALL execute() calls across turns. Each script should do ONE step only: \
         create 1-2 EventNodes, OR temper.list to verify, OR temper.write the skeleton, OR \
         temper.action SeedComplete, OR temper.done — never combine these in one script.\n\
         - After every create batch, run a separate execute() that only calls \
         temper.list(\"EventNodes\", \"world_id eq '{world_id}'\") and prints the count.\n\
         - Only call temper.done(\"complete\") in its own final execute(), after SeedComplete \
         returns successfully.\n\n\
         For each determined fact, create an EventNode:\n\
         temper.create(\"EventNodes\", {{\"world_id\": \"{world_id}\", \"statement\": \"...\", \
         \"layer\": \"slow|mid\", \"probability\": \"1.0\", \"provenance\": \"determined\", \
         \"source_refs\": \"[\\\"<url-or-corpus-ref>\\\"]\", \"resolve_by\": \"YYYY-MM-DD\", \
         \"author_agent_id\": \"{agent_id}\"}})\n\
         Aim for 8-20 load-bearing facts, slow layer first. Every statement needs a source.\n\n\
         THEN name the 3-5 load-bearing UNCERTAINTY AXES of this domain (ADR-006): the \
         dimensions on which {target_date} genuinely FORKS — not determined, the questions \
         whose answers most change the world. For each, give a short axis name and the \
         CONSENSUS POLE (what most informed observers currently expect). These are the \
         dimensions the engine will sample diverse futures across, so make them orthogonal \
         and genuinely contested — not restatements of one another.\n\n\
         Then temper.write(\"/skeleton.md\", \"<markdown summary>\") — the ONLY way to create \
         a file. Do NOT create Files, Directories, or Workspaces yourself.\n\
         temper.write returns {{\"file_id\": \"...\"}}. In a separate execute(), call \
         temper.action(\"Worlds\", \"{world_id}\", \"SeedComplete\", {{\"skeleton_node_count\": \
         \"<n>\", \"graph_snapshot_file_id\": \"<file_id>\", \"uncertainty_axes\": \
         \"[{{\\\"axis\\\": \\\"...\\\", \\\"consensus_pole\\\": \\\"...\\\"}}, ...]\"}}).\n\
         Then temper.done(\"complete\") in its own execute()."
    )
}

/// Open exploration starts with a research map, not a set of fixed commitments.
/// Keep the legacy corridor prompt separate: its sampler requires a skeleton.
fn open_research_prompt(
    world_id: &str,
    agent_id: &str,
    question: &str,
    target_date: &str,
    as_of_date: &str,
    corpus_inline: &str,
    hindcast: bool,
) -> String {
    let research = if hindcast {
        "HINDCAST: NO web access. Use only the frozen corpus, respecting its vantage date.          Later knowledge is inadmissible, even if you remember it. Record missing evidence."
    } else {
        "Use temper.web_search and temper.web_fetch to investigate the question with current          evidence. Follow surprising findings and competing explanations. Inspect source          content before citing it; search snippets alone are leads, not verified findings.          Seek evidence that could overturn your emerging account, not just confirm it. Prefer direct temper.web_fetch(url). If direct fetch fails, temper.web_search(query) returns bounded source-extracted text in each result's text field. A focused title or site query may retrieve a useful excerpt. Use only a claim and quotation actually contained in that returned text; never infer source contents from the title, URL or a search summary. Explicitly label indexed-excerpt evidence, direct-fetch failure and date or context limitations in the statement/evidence_note; use weak_signal when context remains unverified. A truncated excerpt does not establish that the whole source was inspected. Smaller article or text-version URLs may be fetched only when actually discovered, never invented. web_fetch accepts only a URL; do not invent size, range or encoding parameters."
    };
    let chronology = if hindcast {
        "The frozen corpus vantage date is authoritative. The live ingestion date does not move that boundary.".to_string()
    } else if as_of_date.trim().is_empty() {
        "Research as-of date is unavailable. Establish and report source publication and observation dates; do not invent a current date.".to_string()
    } else {
        format!(
            "Research as-of date: {as_of_date}. This is the evidence vantage, distinct from the forecast horizon. Seek the most recent available relevant evidence and check whether newer findings change older accounts, without an arbitrary lookback window. Older sources may establish historical baselines; label them historical rather than current. State when the latest evidence is unavailable or could not be verified, and distinguish publication dates from dates of the events observed."
        )
    };
    format!(
        r#"You are investigating this question for world {world_id}:
{question}
Horizon: {target_date}
{chronology}

Establish what is ALREADY happening at the research as-of date, in the setting the user
actually means. Check recent first-hand demonstrations, current product behavior, and
observed practice, as well as research. Separate current observations from user-supplied
assumptions and unknowns. Do not mistake an older study or an adoption average for the
present at the frontier. Do not universalize a user's own workflow to everyone. If a tool
or habit already exists in this setting, verify what it already does, where it is used and
its current limits. Record what the evidence establishes today and what remains unknown.
This phase establishes the present. Later world generation proposes imagined futures and
works backward from them; do not create your own inferred future EventNodes here.

Build an open research map that can support genuinely different causal futures. Let the
question and discovered evidence determine what to investigate. There is no prescribed
topic checklist, axis count, branch count, consensus pole, or required narrative. Revise
your framing when evidence points somewhere unexpected. A dated commitment is one kind
of evidence, not the boundary of the research. Explore observed changes, contested claims,
weak signals, counterexamples and emerging mechanisms when they bear on this question.
Distinguish an observation from its interpretation and from a hypothesis about the future.
A source's prediction is evidence that the source made that prediction, not that it is true.
Identify competing causal explanations of observed findings and unanswered research
questions. Preserve contradictions, uncertainty, missing evidence and the reasons a finding
could mislead us.
Do not turn a disagreement or an unsourced possibility into an established fact.

Look beyond announcements and company talking points. When relevant, seek what people
actually do, pay for, struggle with or stop needing, alongside studies and direct evidence.
Do not assume today's jobs, tools and habits survive unchanged. Follow evidence that could
make something disappear or become ordinary, and look for reasons that change might fail.
Those are questions to investigate, not required conclusions or a fixed list of futures.
Write findings in plain language with concrete people, actions, dates and limits. Translate
necessary technical terms. Avoid corporate jargon and news-digest summaries. Describe what
people actually do now and the limits of the evidence. Leave imagined everyday scenes,
future outcomes and proposed paths to the later world-generation phase.

{research}
--- BEGIN CORPUS ---
{corpus_inline}
--- END CORPUS ---

Persist useful research as it emerges so the person can watch it build. Use the exact API:
temper.create("EventNodes", {{"world_id": "{world_id}", "statement": "<what the source actually establishes, with its date, scope and limits>", "layer": "mid", "probability": "", "provenance": "observed", "source_refs": "[\"<URL or corpus reference with a short supporting quotation and source date>\"]", "resolve_by": "{target_date}", "author_agent_id": "{agent_id}"}})
For each sourced node also pass evidence_json, a JSON-encoded object with exactly:
{{"kind":"finding","publication_date":null,"observation_period":{{"start":null,"end":null}},"retrieved_at":null}}.
Each typed record has exactly one source_refs entry; split different sources into separate
records so publication chronology has an unambiguous source.
A finding contains substantive content actually read in a source or returned extracted
excerpt. A title, source existence, report contents list or missing result is a lead,
retained for further research but cannot establish the substantive present baseline.
Publication and observation dates accept YYYY, YYYY-Q1 through YYYY-Q4 for calendar
quarters, YYYY-MM or YYYY-MM-DD at the precision actually supported. Do not treat a
fiscal quarter as a calendar quarter; preserve its wording in the statement and use
null dates unless its calendar bounds are known. Retrieval accepts YYYY-MM-DD. Unknown dates are null, never guessed
from the current vantage or URL. These are different dates: a paper retrieved today can
report a study from years ago. Search published_at is provider-reported metadata; check
against the source before treating it as its publication date. No dated metadata makes a
source true or a forecast observed. A report published now may project outcomes years ahead:
those are forecast horizons, never observation_period dates. State explicitly
what the source projects and keep its forecast horizon in the statement. Use null
observation dates when no historical observation period is supplied; never put the future
horizon in observation_period. A sourced forecast is a report of what its source predicts,
not an observed future outcome; retain its attribution, assumptions and horizon.
Use provenance observed, contested, or weak_signal to identify the source claim's
status. Use determined only for an actually fixed fact. Leave probability empty for research
claims: unknown does not mean 0.5 and sourced does not mean 1.0. A genuinely quoted,
quantified forecast may use provenance market or authored with its stated probability;
identify whose estimate it is, its horizon and its conditions in the statement.
An observed, contested or weak_signal statement must not append your inference about
future work, income or behavior. Do not save inferred future hypotheses in this research
phase, even when they feel obvious. Preserve unresolved questions and missing evidence in
the research map instead. Do not invent findings to fill gaps or a node count; if a source
cannot be inspected, record that limitation honestly. Preserve enough actual source content
to let subsequent evaluations assess what was observed. Across all findings, quote no more than 25 words total
from any one source URL; additional findings can paraphrase with the source reference. Do not pad the map to a target count or collapse
conflicting observations to one consensus. Group only genuinely redundant findings.

The session uses a Monty execute REPL. Work in incremental execute calls: research, persist
findings, inspect the saved EventNodes, then continue investigating. Avoid one monolithic
script; check action results before moving on. You have an operational turn budget, so leave
time to save partial findings and report remaining research questions honestly.
Verify saved nodes with temper.list("EventNodes", "world_id eq '{world_id}'").
Write the research map, competing explanations of observations, evidence limitations and
unanswered questions using temper.write("/skeleton.md", "<research map markdown>").
This legacy filename is storage only; it does not constrain the substance of the research.
Do NOT create Files, Directories, or Workspaces yourself. Save the returned file_id.
Then in a separate execute call use:
temper.action("Worlds", "{world_id}", "SeedComplete", {{"skeleton_node_count": "<actual saved count>", "graph_snapshot_file_id": "<returned file_id>", "uncertainty_axes": "[]"}})
The empty legacy uncertainty_axes field deliberately does not preselect the futures.
Only after SeedComplete succeeds, call temper.done("complete") in its own execute call.
"#
    )
}

fn open_exploration_requested(state: &Value) -> bool {
    state
        .get("booleans")
        .and_then(|fields| fields.get("semantic_exploration_requested"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The bookmaker's working contract: import live market-priced questions as
/// EventNodes. Enrichment only — it never reports world completion.
fn bookmaker_prompt(
    world_id: &str,
    agent_id: &str,
    domain: &str,
    target_date: &str,
    corpus_inline: &str,
    hindcast: bool,
) -> String {
    let source_line = if hindcast {
        let corpus_block = if corpus_inline.is_empty() {
            "No corpus was provided".to_string()
        } else {
            format!(
                "The frozen corpus is inlined below between BEGIN CORPUS and END \
                 CORPUS.\n\n--- BEGIN CORPUS ---\n{corpus_inline}\n--- END CORPUS ---"
            )
        };
        format!(
            "This is a HINDCAST world: no web access. If the frozen corpus contains recorded \
             market prices, import those; otherwise create nothing and finish.\n{corpus_block}"
        )
    } else {
        "Search public prediction markets (Polymarket, Kalshi, Metaculus) with \
         temper.web_search / temper.web_fetch for questions resolving before the target \
         date that bear on this domain."
            .to_string()
    };
    format!(
        "You are the Bookmaker for world {world_id} (domain: {domain}, target: {target_date}).\n\n\
         {source_line}\n\n\
         For each relevant market question, create an EventNode with the market's current \
         price as the probability:\n\
         temper.create(\"EventNodes\", {{\"world_id\": \"{world_id}\", \"statement\": \"<the \
         market question, verbatim>\", \"layer\": \"mid\", \"probability\": \"<0.00-1.00>\", \
         \"provenance\": \"market\", \"source_refs\": \"[\\\"<market-url>\\\"]\", \
         \"resolve_by\": \"YYYY-MM-DD\", \"author_agent_id\": \"{agent_id}\"}})\n\
         Import at most 10. Markets are enrichment: if none exist or fetches fail, that is \
         fine — never invent prices. When done (or stuck), call temper.done(\"complete\") \
         with a one-line summary."
    )
}

/// Workspace name for a world — the cross-module rendezvous key. Every
/// session-spawning module must resolve the same per-world workspace by
/// this exact name.
fn workspace_name(world_id: &str) -> String {
    format!("world-{world_id}")
}

/// Resolve (or create) the per-world PawFS workspace and return its id.
/// Sessions are Configured with this id so temper.write lands inside a
/// workspace PawFS Cedar accepts — without it, File create is denied
/// (resource.workspaceId must match the principal's workspace).
fn ensure_world_workspace(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    world_id: &str,
) -> Result<String, String> {
    let name = workspace_name(world_id);
    // Workspace rows are not readable by agent principals (paw-fs Cedar has
    // no read/list permit on Workspace), so idempotent lookup is impossible:
    // create one per spawn batch. Correctness needs only that each session's
    // Configure workspace matches the files it writes; file READS are
    // unrestricted, so cross-session reads work across workspaces.
    let create_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Workspaces"),
        headers,
        &json!({ "name": name, "quota_limit": "104857600" }).to_string(),
    )?;
    if create_resp.status < 200 || create_resp.status >= 300 {
        return Err(format!(
            "create Workspace {name} failed (HTTP {})",
            create_resp.status
        ));
    }
    serde_json::from_str::<Value>(&create_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| format!("Workspace {name} create returned no entity_id"))
}

#[allow(clippy::too_many_arguments)]
fn spawn_session(
    ctx: &Context,
    api: &str,
    headers: &[(String, String)],
    name: &str,
    role: &str,
    model: &str,
    provider: &str,
    provider_options_json: &str,
    tools: &str,
    max_turns: &str,
    user_message: &str,
    workspace_id: &str,
) -> Result<String, String> {
    let agent_body = json!({ "Name": name, "Role": role });
    let agent_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Agents"),
        headers,
        &agent_body.to_string(),
    )?;
    if agent_resp.status < 200 || agent_resp.status >= 300 {
        return Err(format!(
            "create Agent {name} failed (HTTP {})",
            agent_resp.status
        ));
    }
    let agent_id = serde_json::from_str::<Value>(&agent_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Agent create returned no entity_id")?;

    let session_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions"),
        headers,
        &json!({ "agent_id": agent_id }).to_string(),
    )?;
    if session_resp.status < 200 || session_resp.status >= 300 {
        return Err(session_create_error(
            name,
            session_resp.status,
            &session_resp.body,
        ));
    }
    let session_id = serde_json::from_str::<Value>(&session_resp.body)
        .ok()
        .and_then(|v| {
            v.get("entity_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or("Session create returned no entity_id")?;

    // The prompt needs the real agent id for author fields; substitute now.
    let message = user_message.replace("{AGENT_ID}", &agent_id);
    let configure_body = json!({
        "model": model,
        "provider": provider,
        "provider_options_json": provider_options_json,
        "agent_name": role,
        "tools_enabled": tools,
        "tool_choice": "required",
        "max_turns": max_turns,
        "user_message": message,
        "sandbox_url": "none",
        "workspace_id": workspace_id,
        "temper_api_url": api
    });
    let configure_resp = ctx.http_call(
        "POST",
        &format!("{api}/tdata/Sessions('{session_id}')/TemperPaw.Configure"),
        headers,
        &configure_body.to_string(),
    )?;
    if configure_resp.status < 200 || configure_resp.status >= 300 {
        return Err(format!(
            "Configure for {name} failed (HTTP {}): {}",
            configure_resp.status,
            &configure_resp.body[..configure_resp.body.len().min(200)]
        ));
    }
    ctx.log(
        "info",
        &format!("seed_world: spawned {role} agent {agent_id} session {session_id}"),
    );
    Ok(session_id)
}

fn continuation_prompt(prompt: String, world_id: &str, attempt: u64) -> String {
    if attempt <= 1 {
        return prompt;
    }
    format!(
        "This is research recovery attempt {attempt} for the SAME world. A previous researcher stopped without the application's SeedComplete callback. First inspect temper.list(\"EventNodes\", \"world_id eq '{world_id}'\") and try temper.read(\"/skeleton.md\") in the existing workspace. Preserve useful saved findings; do not recreate them or repeat completed searches. If the file is absent, reconstruct the map from saved nodes. Continue genuinely missing research only, then verify saved nodes, write the map and invoke SeedComplete exactly as specified below. Text describing a tool call is not an executed tool call. Check the actual action result before declaring completion. Never infer completion from node count.\n\n{prompt}"
    )
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let research_attempt = ctx
            .entity_state
            .get("counters")
            .and_then(|counters| counters.get("research_attempt"))
            .and_then(Value::as_u64)
            .filter(|attempt| *attempt > 0)
            .ok_or("World research attempt is missing")?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let get = |k: &str| -> String {
            fields
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let world_id = ctx.entity_id.clone();
        let model = get("agent_model");
        let provider = get("agent_provider");
        if model.trim().is_empty() || provider.trim().is_empty() {
            return Err("World.agent_model and World.agent_provider are required".to_string());
        }
        let hindcast = get("hindcast_mode") == "true";
        let tools = tools_enabled(hindcast);

        let api = ctx
            .config
            .get("temper_api_url")
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        let headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-tenant-id".to_string(), ctx.tenant.clone()),
            ("x-temper-principal-kind".to_string(), "agent".to_string()),
            ("x-temper-principal-id".to_string(), world_id.clone()),
            ("x-temper-agent-type".to_string(), "system".to_string()),
        ];

        // One workspace per world: every session this module spawns writes
        // its files there. Without it, temper.write fails Cedar — hard error.
        let workspace_id = ensure_world_workspace(&ctx, &api, &headers, &world_id)?;

        let corpus_inline = fetch_corpus(&ctx, &api, &headers, &get("corpus_file_id"))?;

        let open_exploration = open_exploration_requested(&ctx.entity_state);
        let surveyor_msg = if open_exploration {
            open_research_prompt(
                &world_id,
                "{AGENT_ID}",
                &get("description"),
                &get("target_date"),
                &get("last_ingest_date"),
                &corpus_inline,
                hindcast,
            )
        } else {
            surveyor_prompt(
                &world_id,
                "{AGENT_ID}",
                &get("name"),
                &get("domain"),
                &get("description"),
                &get("target_date"),
                &corpus_inline,
                hindcast,
            )
        };
        let surveyor_msg = continuation_prompt(surveyor_msg, &world_id, research_attempt);
        let research_session_id = spawn_session(
            &ctx,
            &api,
            &headers,
            &format!("Surveyor-{world_id}"),
            "surveyor",
            &model,
            &provider,
            &get("agent_provider_options_json"),
            &tools,
            if open_exploration { "64" } else { "40" },
            &surveyor_msg,
            &workspace_id,
        )?;

        ctx.log(
            "info",
            "seed_world: done (surveyor will report SeedComplete; bookmaker disabled)",
        );
        set_success_result(
            "ResearchSessionStarted",
            &json!({
                "research_session_id": research_session_id,
                "research_session_attempt": research_attempt.to_string(),
                "expected_research_attempt": research_attempt,
            }),
        );
        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_denial_retains_bounded_reason_and_decision_not_headers() {
        let body = serde_json::json!({"error":{"code":"Forbidden","message":"no matching permit","decision_id":"PD-123"},"headers":{"authorization":"secret-not-for-output"},"request":{"token":"secret-not-for-output"}}).to_string();
        let error = super::session_create_error("surveyor", 403, &body);
        assert!(error.contains("HTTP 403"));
        assert!(error.contains("no matching permit"));
        assert!(error.contains("PD-123"));
        assert!(!error.contains("secret-not-for-output"));
        let long = serde_json::json!({"message":"é".repeat(2000)}).to_string();
        assert!(
            super::session_create_error("surveyor", 500, &long)
                .chars()
                .count()
                < 600
        );
        assert_eq!(
            super::session_create_error("surveyor", 502, "private non-JSON body"),
            "create Session for surveyor failed (HTTP 502)"
        );
    }

    use super::*;

    // Prompt-contract tests: the generated prompts must reference the exact
    // entity sets, action names, and parameter names the specs declare.
    // The API silently drops unknown fields — drift here is a silent failure.

    #[test]
    fn research_recovery_preserves_saved_work_before_requesting_completion() {
        assert_eq!(continuation_prompt("original".into(), "w", 1), "original");
        let resumed = continuation_prompt("original".into(), "w", 2);
        assert!(resumed.contains("world_id eq 'w'"));
        assert!(resumed.contains("/skeleton.md"));
        assert!(resumed.contains("do not recreate"));
        assert!(resumed.contains("SeedComplete"));
    }
    #[test]
    fn open_research_keeps_uncertain_findings_without_fixed_axes_or_certainty() {
        let prompt = open_research_prompt(
            "world-a",
            "agent-a",
            "What changes?",
            "2027-01-01",
            "2026-09-19",
            "",
            false,
        );
        for required in [
            "temper.web_search",
            "temper.web_fetch",
            "competing causal explanations",
            "observed, contested, or weak_signal",
            "Leave probability empty",
            "\"uncertainty_axes\": \"[]\"",
            "SeedComplete",
        ] {
            assert!(prompt.contains(required), "missing {required}");
        }
        for obsolete in [
            "ALREADY DETERMINED",
            "forbidden from predicting",
            "Aim for 8-20",
            "3-5 load-bearing",
            "\"probability\": \"1.0\"",
        ] {
            assert!(
                !prompt.contains(obsolete),
                "research regressed to {obsolete}"
            );
        }
    }

    #[test]
    fn world_first_research_does_not_generate_future_candidates() {
        let prompt = open_research_prompt("w", "a", "Any question", "2030", "2026", "", false);
        assert!(prompt.contains("do not create your own inferred future EventNodes here"));
        assert!(prompt.contains("A sourced forecast is a report of what its source predicts"));
        assert!(prompt.contains("never observation_period dates"));
        assert!(!prompt.contains("Save that inference separately with provenance hypothesis"));
        assert!(!prompt.contains("novel hypotheses worth testing"));
        assert!(!prompt.contains("For each inferred future"));
    }

    #[test]
    fn open_research_has_bounded_excerpt_fallback_without_invented_fetch_parameters() {
        let prompt = open_research_prompt("w", "a", "question", "2027", "2026", "", false);
        for required in [
            "result's text field",
            "actually contained in that returned text",
            "indexed-excerpt evidence",
            "weak_signal",
            "only when actually discovered",
            "no more than 25 words total",
        ] {
            assert!(prompt.contains(required), "missing {required}");
        }
        let frozen = open_research_prompt("w", "a", "question", "2020", "2026", "corpus", true);
        assert!(!frozen.contains("focused title or site query"));
    }

    #[test]
    fn live_research_distinguishes_evidence_vantage_from_forecast_horizon() {
        let prompt =
            open_research_prompt("w", "a", "question", "2027-09-19", "2026-09-19", "", false);
        assert!(prompt.contains("Horizon: 2027-09-19"));
        assert!(prompt.contains("Research as-of date: 2026-09-19"));
        assert!(prompt.contains("label them historical rather than current"));
        assert!(prompt.contains("latest evidence is unavailable"));
        let frozen = open_research_prompt(
            "w",
            "a",
            "question",
            "2020-01-01",
            "2026-09-19",
            "corpus vantage 2019-01-01",
            true,
        );
        assert!(!frozen.contains("2026-09-19"));
        assert!(frozen.contains("corpus vantage 2019-01-01"));
        assert!(frozen.contains("frozen corpus vantage date is authoritative"));
    }

    #[test]
    fn open_hindcast_never_offers_live_web_and_preserves_corpus() {
        let prompt = open_research_prompt(
            "w",
            "a",
            "question",
            "2020-01-01",
            "2026-09-19",
            "frozen evidence",
            true,
        );
        assert!(prompt.contains("NO web access"));
        assert!(prompt.contains("frozen evidence"));
        assert!(!prompt.contains("temper.web_search"));
        assert!(!prompt.contains("temper.web_fetch"));
    }

    #[test]
    fn semantic_request_selects_open_research_without_changing_corridors() {
        assert!(open_exploration_requested(
            &json!({"booleans": {"semantic_exploration_requested": true}})
        ));
        assert!(!open_exploration_requested(
            &json!({"booleans": {"semantic_exploration_requested": false}})
        ));
        assert!(!open_exploration_requested(&json!({})));
    }

    #[test]
    fn surveyor_prompt_carries_the_event_node_and_seed_complete_contract() {
        let p = surveyor_prompt(
            "w-1",
            "a-1",
            "Test",
            "ai coding tools",
            "desc",
            "2026-12-11",
            "frozen corpus body text",
            false,
        );
        for needle in [
            "temper.create(\"EventNodes\"",
            "\"world_id\": \"w-1\"",
            "\"provenance\": \"determined\"",
            "\"author_agent_id\": \"a-1\"",
            "temper.action(\"Worlds\", \"w-1\", \"SeedComplete\"",
            "skeleton_node_count",
            "graph_snapshot_file_id",
            "BEGIN CORPUS",
            "frozen corpus body text",
            "forbidden from predicting",
            // ADR-006: the surveyor also names the uncertainty axes that steer
            // the diverse worlds, reported in SeedComplete.
            "UNCERTAINTY AXES",
            "consensus_pole",
            "\"uncertainty_axes\":",
        ] {
            assert!(p.contains(needle), "surveyor prompt missing: {needle}");
        }
        // Sessions cannot resolve harness-uploaded files by id (path+workspace
        // resolution) — the corpus must be inlined, never temper.read.
        assert!(
            !p.contains("temper.read("),
            "surveyor prompt must not ask the session to read the corpus"
        );
        assert!(
            p.contains("NEVER put the whole job in one execute()"),
            "surveyor prompt must forbid monolithic execute() scripts"
        );
        assert!(
            p.contains("MANY SMALL execute()"),
            "surveyor prompt must require incremental execute() calls"
        );
    }

    #[test]
    fn surveyor_prompt_gives_explicit_file_write_recipe() {
        // Same class of bug as the adversary wedge: an under-specified file-write
        // instruction lets a session reverse-engineer file creation through
        // temper.action against Directories, trip a Cedar gate, and loop in
        // WaitingForApproval. The prompt must show the exact temper.write call,
        // name the file_id return field, forbid improvising file/dir creation,
        // and never leave the bare placeholder behind. (The surveyor DOES create
        // EventNodes via temper.create — the prohibition is scoped to
        // Files/Directories/Workspaces only.)
        let p = surveyor_prompt(
            "w-1",
            "a-1",
            "Test",
            "ai coding tools",
            "desc",
            "2026-12-11",
            "corpus",
            false,
        );
        assert!(
            p.contains("temper.write(\"/skeleton.md\""),
            "surveyor prompt must show the literal temper.write call"
        );
        assert!(
            p.contains("\"file_id\""),
            "surveyor prompt must name the file_id return field"
        );
        assert!(
            p.contains("Do NOT") && p.contains("Directories"),
            "surveyor prompt must forbid improvising Directories file creation"
        );
        assert!(
            !p.contains("<file-id-from-temper.write>"),
            "the bare placeholder must be gone — the recipe captures result[\"file_id\"]"
        );
    }

    #[test]
    fn bookmaker_prompt_imports_market_provenance_only() {
        let p = bookmaker_prompt("w-1", "a-1", "ai coding tools", "2026-12-11", "", false);
        for needle in [
            "temper.create(\"EventNodes\"",
            "\"provenance\": \"market\"",
            "never invent prices",
        ] {
            assert!(p.contains(needle), "bookmaker prompt missing: {needle}");
        }
        assert!(
            !p.contains("SeedComplete"),
            "bookmaker must not report world completion"
        );
    }

    #[test]
    fn hindcast_bookmaker_sees_the_corpus_inline() {
        let p = bookmaker_prompt(
            "w-1",
            "a-1",
            "ai coding tools",
            "2025-12-11",
            "recorded market prices live here",
            true,
        );
        assert!(p.contains("BEGIN CORPUS"));
        assert!(p.contains("recorded market prices live here"));
        assert!(!p.contains("temper.read("));
    }

    #[test]
    fn corpus_inlining_truncates_loudly_at_cap() {
        let big = "x".repeat(40_000);
        let inlined = inline_corpus(&big);
        assert!(inlined.len() < 31_000);
        assert!(inlined.contains("TRUNCATED AT 30KB"));
        let small = inline_corpus("tiny");
        assert_eq!(small, "tiny");
    }

    #[test]
    fn workspace_name_is_the_cross_module_rendezvous_key() {
        // All six session-spawning modules must derive the exact same
        // workspace name from a world id, or their sessions write into
        // different workspaces.
        assert_eq!(workspace_name("w-1"), "world-w-1");
        assert_eq!(workspace_name("0197abc"), "world-0197abc");
    }

    #[test]
    fn hindcast_mode_strips_web_tools_everywhere() {
        assert!(!tools_enabled(true).contains("web"));
        assert!(tools_enabled(false).contains("temper_web_search"));
        let p = surveyor_prompt("w", "a", "n", "d", "", "2026-01-01", "", true);
        assert!(p.contains("NO web access"));
        assert!(!p.contains("temper.web_search /"));
        let b = bookmaker_prompt("w", "a", "d", "2026-01-01", "", true);
        assert!(b.contains("no web access"));
    }
}
