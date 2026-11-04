//! Who a transport acts for.
//!
//! A transport takes a message, a command or a button press only from an owner
//! of its bot. The provider that issued the bot's token says who that is, so
//! there is nothing to configure and a change of owner holds at once. When the
//! provider gives no clear answer, nobody is admitted, and a refused sender is
//! told so in plain words.
//!
//! [`Owners::admit`] is the one check. The moves it guards ask for its [`Owner`]
//! proof ([`crate::PawApiClient::receive_message`], [`apply_press`]), so a new
//! transport has to pass through it to reach them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::discord::gateway::DISCORD_API_BASE;
use crate::slack::api::SLACK_API_BASE;
use crate::{
    PawApiClient, approval_body_for_scope, approval_scope_from_action, fetch_pending_decision,
};

type ProviderCall<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Answers whether one provider user id owns this bot.
pub type OwnerLookup = Arc<dyn Fn(String) -> ProviderCall<bool> + Send + Sync>;

/// Says something to a provider channel: `(channel_id, text)`.
pub type ChannelTell = Arc<dyn Fn(String, &'static str) -> ProviderCall<()> + Send + Sync>;

/// Discord drops an interaction not answered within 3 seconds, and this runs
/// before the answer.
const LOOKUP_BUDGET: Duration = Duration::from_secs(2);

/// Discord's `membership_state` for someone who accepted a team invitation.
const TEAM_MEMBER_ACCEPTED: u8 = 2;

/// Why a sender was not admitted, with what they are told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotOwner,
    OwnersUnknown,
}

impl Refusal {
    pub fn answer(self) -> &'static str {
        match self {
            Self::NotOwner => "This bot only takes requests from its owner.",
            Self::OwnersUnknown => {
                "I can't confirm who owns this bot right now, so I can't take this. Please try again shortly."
            }
        }
    }
}

/// Proof that a sender owns this bot. Only [`Owners::admit`] makes one.
#[derive(Debug)]
pub struct Owner {
    id: String,
}

impl Owner {
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The check every transport's way in passes through.
#[derive(Clone)]
pub struct Owners {
    lookup: OwnerLookup,
    tell: ChannelTell,
}

impl std::fmt::Debug for Owners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owners").finish_non_exhaustive()
    }
}

impl Owners {
    pub fn new(lookup: OwnerLookup, tell: ChannelTell) -> Self {
        Self { lookup, tell }
    }

    /// Discord's application record: the owner of a personal app, or the owner
    /// and admins of a team's. Discord describes a team admin as having nearly
    /// the owner's access; a developer or read-only member is not an owner.
    pub fn discord(http: reqwest::Client, bot_token: String) -> Self {
        Self::discord_at(http, bot_token, DISCORD_API_BASE.to_string())
    }

    fn discord_at(http: reqwest::Client, bot_token: String, api_base: String) -> Self {
        let authorization = format!("Bot {bot_token}");
        let (lookup_http, lookup_authorization, lookup_base) =
            (http.clone(), authorization.clone(), api_base.clone());
        let lookup: OwnerLookup = Arc::new(move |sender_id| {
            let request = lookup_http
                .get(format!("{lookup_base}/applications/@me"))
                .header("Authorization", &lookup_authorization);
            Box::pin(async move {
                let response = request
                    .send()
                    .await
                    .map_err(|error| format!("Discord application lookup failed: {error}"))?;
                let status = response.status();
                if !status.is_success() {
                    return Err(format!("Discord application lookup returned {status}"));
                }
                let application: DiscordApplication = response
                    .json()
                    .await
                    .map_err(|error| format!("Discord application record unreadable: {error}"))?;
                application.is_owned_by(&sender_id)
            })
        });
        let tell: ChannelTell = Arc::new(move |channel_id, text| {
            let request = http
                .post(format!("{api_base}/channels/{channel_id}/messages"))
                .header("Authorization", &authorization)
                .json(&serde_json::json!({ "content": text }));
            Box::pin(async move {
                let response = request.send().await.map_err(|error| error.to_string())?;
                let status = response.status();
                if status.is_success() {
                    Ok(())
                } else {
                    Err(format!("Discord returned {status}"))
                }
            })
        });
        Self::new(lookup, tell)
    }

    /// Slack's user record: an owner of the workspace the bot is installed in.
    /// A bot token needs the `users:read` scope to read it.
    pub fn slack(http: reqwest::Client, bot_token: String) -> Self {
        Self::slack_at(http, bot_token, SLACK_API_BASE.to_string())
    }

    fn slack_at(http: reqwest::Client, bot_token: String, api_base: String) -> Self {
        let authorization = format!("Bearer {bot_token}");
        let (lookup_http, lookup_authorization, lookup_base) =
            (http.clone(), authorization.clone(), api_base.clone());
        let lookup: OwnerLookup = Arc::new(move |sender_id| {
            let request = lookup_http
                .get(format!("{lookup_base}/users.info"))
                .query(&[("user", sender_id.as_str())])
                .header("Authorization", &lookup_authorization);
            Box::pin(async move {
                let response = request
                    .send()
                    .await
                    .map_err(|error| format!("Slack user lookup failed: {error}"))?;
                let answer: SlackUserInfo = response
                    .json()
                    .await
                    .map_err(|error| format!("Slack user record unreadable: {error}"))?;
                answer.is_owner()
            })
        });
        let tell: ChannelTell = Arc::new(move |channel_id, text| {
            let request = http
                .post(format!("{api_base}/chat.postMessage"))
                .header("Authorization", &authorization)
                .json(&serde_json::json!({ "channel": channel_id, "text": text }));
            Box::pin(async move {
                let response = request.send().await.map_err(|error| error.to_string())?;
                let answer: SlackPosted =
                    response.json().await.map_err(|error| error.to_string())?;
                match answer.ok {
                    true => Ok(()),
                    false => Err(answer.error.unwrap_or_else(|| "unknown".to_string())),
                }
            })
        });
        Self::new(lookup, tell)
    }

    /// Admit `sender_id` if the provider says they own this bot. Anything short
    /// of a clear yes refuses, and the refusal says which it was.
    pub async fn admit(&self, sender_id: &str) -> Result<Owner, Refusal> {
        if sender_id.is_empty() {
            return Err(Refusal::NotOwner);
        }
        let answer =
            tokio::time::timeout(LOOKUP_BUDGET, (self.lookup)(sender_id.to_string())).await;
        match answer {
            Ok(Ok(true)) => Ok(Owner {
                id: sender_id.to_string(),
            }),
            Ok(Ok(false)) => {
                tracing::info!(sender_id, "transport refused a sender who is not an owner");
                Err(Refusal::NotOwner)
            }
            Ok(Err(error)) => {
                tracing::warn!(sender_id, %error, "transport could not confirm the bot's owners");
                Err(Refusal::OwnersUnknown)
            }
            Err(_) => {
                tracing::warn!(sender_id, "transport owner lookup ran out of time");
                Err(Refusal::OwnersUnknown)
            }
        }
    }

    /// [`Self::admit`], telling a refused sender why in `channel_id`. For a way
    /// in that has no reply of its own to answer with.
    pub async fn admit_or_answer(&self, sender_id: &str, channel_id: &str) -> Option<Owner> {
        match self.admit(sender_id).await {
            Ok(owner) => Some(owner),
            Err(refusal) => {
                if let Err(error) = (self.tell)(channel_id.to_string(), refusal.answer()).await {
                    tracing::warn!(%error, "transport refusal could not be delivered");
                }
                None
            }
        }
    }

    /// Say at startup whether the provider answers, using an id that is never
    /// an owner, so a lookup that cannot work shows before anyone is refused.
    pub async fn report_source(&self, transport: &str, bot_user_id: &str) {
        match self.admit(bot_user_id).await {
            Err(Refusal::OwnersUnknown) => {
                tracing::warn!(
                    transport,
                    "transport owner source is not answering; every sender is refused until it does"
                );
            }
            Ok(_) | Err(Refusal::NotOwner) => {
                tracing::info!(transport, "transport owner source is answering");
            }
        }
    }
}

#[derive(Deserialize)]
struct DiscordApplication {
    #[serde(default)]
    owner: Option<DiscordAppUser>,
    #[serde(default)]
    team: Option<DiscordAppTeam>,
}

#[derive(Deserialize)]
struct DiscordAppUser {
    id: String,
}

#[derive(Deserialize)]
struct DiscordAppTeam {
    owner_user_id: String,
    #[serde(default)]
    members: Vec<DiscordAppTeamMember>,
}

#[derive(Deserialize)]
struct DiscordAppTeamMember {
    membership_state: u8,
    #[serde(default)]
    role: String,
    user: DiscordAppUser,
}

impl DiscordApplication {
    fn is_owned_by(&self, sender_id: &str) -> Result<bool, String> {
        match (&self.team, &self.owner) {
            (Some(team), _) => Ok(team.owner_user_id == sender_id
                || team.members.iter().any(|member| {
                    member.membership_state == TEAM_MEMBER_ACCEPTED
                        && member.role == "admin"
                        && member.user.id == sender_id
                })),
            (None, Some(owner)) => Ok(owner.id == sender_id),
            (None, None) => Err("Discord application record names no owner".to_string()),
        }
    }
}

#[derive(Deserialize)]
struct SlackUserInfo {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    user: Option<SlackUserRecord>,
}

#[derive(Deserialize)]
struct SlackPosted {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct SlackUserRecord {
    #[serde(default)]
    is_owner: bool,
    #[serde(default)]
    is_primary_owner: bool,
    #[serde(default)]
    deleted: bool,
}

impl SlackUserInfo {
    fn is_owner(&self) -> Result<bool, String> {
        if !self.ok {
            let error = self.error.as_deref().unwrap_or("unknown");
            return match error {
                "user_not_found" => Ok(false),
                _ => Err(format!("Slack user lookup refused: {error}")),
            };
        }
        let user = self
            .user
            .as_ref()
            .ok_or_else(|| "Slack user record is empty".to_string())?;
        Ok((user.is_owner || user.is_primary_owner) && !user.deleted)
    }
}

/// Apply a button press an owner made: a decision or a plan review. Answers
/// whether it was applied, and the line to show. `provider` names where the
/// press came from, in what the decision records.
pub async fn apply_press(
    api: &PawApiClient,
    by: &Owner,
    provider: &str,
    action: &str,
    target_id: &str,
) -> (bool, String) {
    let reviewer_id = by.id();
    let decided_by = format!("{provider}:{reviewer_id}");
    let base_url = &api.config().base_url;
    let tenant = &api.config().tenant;
    match action {
        approval_action if approval_scope_from_action(approval_action).is_some() => {
            let approve_url =
                format!("{base_url}/api/tenants/{tenant}/decisions/{target_id}/approve");
            let scope =
                approval_scope_from_action(approval_action).expect("checked approval action above");
            let decision = fetch_pending_decision(api, base_url, tenant, target_id)
                .await
                .ok()
                .flatten();
            match approval_body_for_scope(scope, decision.as_ref(), decided_by) {
                Ok(body) => match api.raw_post(&approve_url, body).await {
                    Ok(_) => (true, format!("Approval recorded by <@{reviewer_id}>")),
                    Err(e) => (false, format!("Approval failed: {e}")),
                },
                Err(e) => (false, format!("Approval failed: {e}")),
            }
        }
        "deny" => {
            let deny_url = format!("{base_url}/api/tenants/{tenant}/decisions/{target_id}/deny");
            let deny_body = serde_json::json!({ "decided_by": decided_by });
            match api.raw_post(&deny_url, deny_body).await {
                Ok(_) => (true, format!("Denial recorded by <@{reviewer_id}>")),
                Err(e) => (false, format!("Deny failed: {e}")),
            }
        }
        "plan_approve" => match api
            .dispatch_action(
                "Plans",
                target_id,
                "TemperPaw.Approve",
                serde_json::json!({}),
            )
            .await
        {
            Ok(_) => (true, format!("Plan approved by <@{reviewer_id}>")),
            Err(e) => (false, format!("Plan approval failed: {e}")),
        },
        "plan_request_changes" => {
            let review_notes = format!(
                "Changes requested by {decided_by}. Review the plan, revise it, and resubmit for approval."
            );
            match api
                .dispatch_action(
                    "Plans",
                    target_id,
                    "TemperPaw.RequestChanges",
                    serde_json::json!({ "review_notes": review_notes }),
                )
                .await
            {
                Ok(_) => (
                    true,
                    format!(
                        "Plan changes requested by <@{reviewer_id}>. Additional details can be sent in-thread."
                    ),
                ),
                Err(e) => (false, format!("Request changes failed: {e}")),
            }
        }
        _ => (false, "Unknown action.".to_string()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;

    /// What a transport said to a sender: `(channel_id, text)`.
    pub(crate) type Told = Arc<Mutex<Vec<(String, String)>>>;

    fn says_nothing() -> ChannelTell {
        Arc::new(|_, _| Box::pin(async { Ok(()) }))
    }

    /// Owners by a fixed list, for tests of a transport's ways in.
    pub(crate) fn only(owner: &'static str) -> Owners {
        Owners::new(
            Arc::new(move |sender_id| Box::pin(async move { Ok(sender_id == owner) })),
            says_nothing(),
        )
    }

    /// A provider that gives no answer.
    pub(crate) fn unanswered() -> Owners {
        Owners::new(
            Arc::new(|_| Box::pin(async { Err("no answer".to_string()) })),
            says_nothing(),
        )
    }

    /// The same owners, keeping what they tell a refused sender.
    pub(crate) fn recording(owners: Owners) -> (Owners, Told) {
        let told = Told::default();
        let kept = told.clone();
        let tell: ChannelTell = Arc::new(move |channel_id, text| {
            kept.lock().unwrap().push((channel_id, text.to_string()));
            Box::pin(async { Ok(()) })
        });
        (Owners::new(owners.lookup, tell), told)
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{address}")
    }

    async fn discord_says(application: serde_json::Value) -> Owners {
        let base = serve(Router::new().route(
            "/applications/@me",
            get(move || {
                let application = application.clone();
                async move { Json(application) }
            }),
        ))
        .await;
        Owners::discord_at(reqwest::Client::new(), "token".to_string(), base)
    }

    async fn slack_says(answer: serde_json::Value) -> Owners {
        let base = serve(Router::new().route(
            "/users.info",
            get(move || {
                let answer = answer.clone();
                async move { Json(answer) }
            }),
        ))
        .await;
        Owners::slack_at(reqwest::Client::new(), "token".to_string(), base)
    }

    #[tokio::test]
    async fn an_owner_is_admitted_and_anyone_else_is_told_no() {
        let owners = only("rita");
        assert_eq!(owners.admit("rita").await.unwrap().id(), "rita");
        assert_eq!(
            owners.admit("someone").await.unwrap_err(),
            Refusal::NotOwner
        );
        assert_eq!(owners.admit("").await.unwrap_err(), Refusal::NotOwner);
    }

    #[tokio::test]
    async fn nobody_is_admitted_while_the_provider_gives_no_answer() {
        assert_eq!(
            unanswered().admit("rita").await.unwrap_err(),
            Refusal::OwnersUnknown
        );

        let slow = Owners::new(
            Arc::new(|_| {
                Box::pin(async {
                    tokio::time::sleep(LOOKUP_BUDGET * 2).await;
                    Ok(true)
                })
            }),
            says_nothing(),
        );
        assert_eq!(
            slow.admit("rita").await.unwrap_err(),
            Refusal::OwnersUnknown
        );
    }

    #[tokio::test]
    async fn a_refused_sender_is_told_why_and_an_owner_hears_nothing() {
        let (owners, told) = recording(only("rita"));
        assert!(owners.admit_or_answer("someone", "dm-1").await.is_none());
        assert_eq!(
            told.lock().unwrap().clone(),
            [("dm-1".to_string(), Refusal::NotOwner.answer().to_string())]
        );

        assert!(owners.admit_or_answer("rita", "dm-1").await.is_some());
        assert_eq!(told.lock().unwrap().len(), 1);
    }

    /// Serves a provider's say-something route and keeps the bodies it is sent.
    async fn provider_keeping_messages(
        path: &'static str,
        answer: serde_json::Value,
    ) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let bodies = kept.clone();
        let base = serve(Router::new().route(
            path,
            post(move |Json(body): Json<serde_json::Value>| {
                let bodies = bodies.clone();
                let answer = answer.clone();
                async move {
                    bodies.lock().unwrap().push(body);
                    Json(answer)
                }
            }),
        ))
        .await;
        (base, kept)
    }

    #[tokio::test]
    async fn discord_and_slack_deliver_the_refusal_to_the_senders_channel() {
        let (base, kept) =
            provider_keeping_messages("/channels/dm-1/messages", json!({ "id": "m" })).await;
        let owners = Owners::discord_at(reqwest::Client::new(), "token".to_string(), base);
        assert!(owners.admit_or_answer("111", "dm-1").await.is_none());
        assert_eq!(
            kept.lock().unwrap().clone(),
            [json!({ "content": Refusal::OwnersUnknown.answer() })]
        );

        let (base, kept) =
            provider_keeping_messages("/chat.postMessage", json!({ "ok": true })).await;
        let owners = Owners::slack_at(reqwest::Client::new(), "token".to_string(), base);
        assert!(owners.admit_or_answer("U1", "D1").await.is_none());
        assert_eq!(
            kept.lock().unwrap().clone(),
            [json!({ "channel": "D1", "text": Refusal::OwnersUnknown.answer() })]
        );
    }

    #[tokio::test]
    async fn a_personal_discord_app_is_owned_by_its_owner() {
        let owners = discord_says(json!({ "owner": { "id": "111" }, "team": null })).await;
        assert!(owners.admit("111").await.is_ok());
        assert_eq!(owners.admit("222").await.unwrap_err(), Refusal::NotOwner);
    }

    #[tokio::test]
    async fn a_team_discord_app_is_owned_by_its_owner_and_accepted_admins() {
        let owners = discord_says(json!({
            "owner": { "id": "team-user" },
            "team": {
                "owner_user_id": "111",
                "members": [
                    { "membership_state": 2, "role": "admin", "user": { "id": "222" } },
                    { "membership_state": 1, "role": "admin", "user": { "id": "333" } },
                    { "membership_state": 2, "role": "developer", "user": { "id": "444" } },
                    { "membership_state": 2, "role": "read_only", "user": { "id": "555" } }
                ]
            }
        }))
        .await;
        assert!(owners.admit("111").await.is_ok());
        assert!(owners.admit("222").await.is_ok());
        for someone_else in ["333", "444", "555", "team-user"] {
            assert_eq!(
                owners.admit(someone_else).await.unwrap_err(),
                Refusal::NotOwner,
                "{someone_else}"
            );
        }
    }

    #[tokio::test]
    async fn a_discord_app_record_naming_no_owner_admits_nobody() {
        let owners = discord_says(json!({ "id": "app" })).await;
        assert_eq!(
            owners.admit("111").await.unwrap_err(),
            Refusal::OwnersUnknown
        );
    }

    #[tokio::test]
    async fn a_refused_discord_lookup_admits_nobody() {
        let base = serve(Router::new()).await;
        let owners = Owners::discord_at(reqwest::Client::new(), "token".to_string(), base);
        assert_eq!(
            owners.admit("111").await.unwrap_err(),
            Refusal::OwnersUnknown
        );
    }

    #[tokio::test]
    async fn slack_says_who_owns_the_workspace() {
        let owner =
            slack_says(json!({ "ok": true, "user": { "is_owner": true, "deleted": false } })).await;
        assert!(owner.admit("U1").await.is_ok());

        let primary = slack_says(json!({ "ok": true, "user": { "is_primary_owner": true } })).await;
        assert!(primary.admit("U1").await.is_ok());

        let member =
            slack_says(json!({ "ok": true, "user": { "is_admin": true, "is_owner": false } }))
                .await;
        assert_eq!(member.admit("U1").await.unwrap_err(), Refusal::NotOwner);

        let gone =
            slack_says(json!({ "ok": true, "user": { "is_owner": true, "deleted": true } })).await;
        assert_eq!(gone.admit("U1").await.unwrap_err(), Refusal::NotOwner);

        let unknown = slack_says(json!({ "ok": false, "error": "user_not_found" })).await;
        assert_eq!(unknown.admit("U1").await.unwrap_err(), Refusal::NotOwner);
    }

    #[tokio::test]
    async fn a_slack_token_that_cannot_read_users_admits_nobody() {
        let owners = slack_says(json!({ "ok": false, "error": "missing_scope" })).await;
        assert_eq!(
            owners.admit("U1").await.unwrap_err(),
            Refusal::OwnersUnknown
        );
    }
}
