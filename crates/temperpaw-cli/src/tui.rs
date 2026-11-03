use std::collections::VecDeque;
use std::io::{Write, stdout};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::Args;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use paw_transport::{
    ApprovalScope, PawApiClient, PawApiConfig, approval_body_for_scope, fetch_pending_decision,
};
use serde_json::json;
use tokio::sync::mpsc;

use crate::cli_channel::{ensure_cli_channel, receive_message_params};
use crate::events::{TuiEvent, watch_cli_channel};

const DEFAULT_URL: &str = "http://127.0.0.1:3467";
const DEFAULT_TENANT: &str = "default";
const DEFAULT_PROFILE: &str = "local";
const DEFAULT_SESSION: &str = "main";
const MAX_MESSAGES: usize = 500;

#[derive(Debug, Clone, Args)]
pub struct TuiArgs {
    /// TemperPaw server URL
    #[arg(long, default_value = DEFAULT_URL, env = "TEMPERPAW_URL")]
    pub url: String,

    /// Temper tenant id
    #[arg(long, default_value = DEFAULT_TENANT, env = "TEMPERPAW_TENANT")]
    pub tenant: String,

    /// API key for non-loopback TemperPaw servers
    #[arg(long, env = "TEMPERPAW_API_KEY")]
    pub api_key: Option<String>,

    /// Local CLI profile name used to derive the Channel id
    #[arg(long, default_value = DEFAULT_PROFILE)]
    pub profile: String,

    /// Conversation session key within the CLI channel
    #[arg(long, default_value = DEFAULT_SESSION)]
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct Message {
    role: Role,
    content: String,
}

struct AppState {
    base_url: String,
    tenant: String,
    profile: String,
    channel_entity_id: String,
    session_key: String,
    status: String,
    input: String,
    messages: VecDeque<Message>,
}

pub async fn run(args: TuiArgs) -> anyhow::Result<()> {
    validate_key("profile", &args.profile)?;
    validate_key("session", &args.session)?;

    let base_url = normalize_url(&args.url);
    let api = PawApiClient::new(PawApiConfig {
        base_url: base_url.clone(),
        tenant: args.tenant.clone(),
        api_key: args.api_key.clone(),
    });

    let channel = ensure_cli_channel(&api, &args.profile)
        .await
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to prepare cli Channel on {base_url}"))?;

    let active_thread = Arc::new(Mutex::new(args.session.clone()));
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(watch_cli_channel(
        api.clone(),
        channel.entity_id.clone(),
        active_thread.clone(),
        tx,
    ));

    let mut app = AppState {
        base_url,
        tenant: args.tenant,
        profile: args.profile,
        channel_entity_id: channel.entity_id,
        session_key: args.session,
        status: "idle".to_string(),
        input: String::new(),
        messages: VecDeque::new(),
    };
    app.push_system("Connected. Type /help for commands.");

    let _terminal = TerminalGuard::enter()?;
    let mut stdout = stdout();

    loop {
        while let Ok(event) = rx.try_recv() {
            apply_event(&mut app, event);
        }

        render(&mut stdout, &app)?;

        if event::poll(std::time::Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) if should_exit_immediately(&key) => break,
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    let line = app.input.trim().to_string();
                    app.input.clear();
                    if !handle_line(&api, &mut app, &active_thread, line).await? {
                        break;
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Backspace,
                    ..
                }) => {
                    app.input.pop();
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Char(c),
                    modifiers,
                    ..
                }) if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                    app.input.push(c);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

async fn handle_line(
    api: &PawApiClient,
    app: &mut AppState,
    active_thread: &Arc<Mutex<String>>,
    line: String,
) -> anyhow::Result<bool> {
    if line.is_empty() {
        return Ok(true);
    }

    match parse_command(&line) {
        ParsedLine::Exit => return Ok(false),
        ParsedLine::Help => {
            app.push_system(
                "/help /status /plan <task> /execute <task> /reset [message] /session <key> /approve-always <decision> /approve-session <decision> /approve-once <decision> /deny <decision> /plan-approve <plan> /request-changes <plan> [notes] /exit",
            );
            return Ok(true);
        }
        ParsedLine::Status => {
            app.push_system(format!(
                "server={} tenant={} profile={} channel={} session={} status={}",
                app.base_url,
                app.tenant,
                app.profile,
                app.channel_entity_id,
                app.session_key,
                app.status
            ));
            return Ok(true);
        }
        ParsedLine::Session(next) => {
            if let Err(error) = validate_key("session", &next) {
                app.push_system(error.to_string());
                return Ok(true);
            }
            app.session_key = next.clone();
            if let Ok(mut thread) = active_thread.lock() {
                *thread = next.clone();
            }
            app.push_system(format!("Switched to session `{next}`."));
            return Ok(true);
        }
        ParsedLine::Unknown(command) => {
            app.push_system(format!("Unknown command `{command}`. Type /help."));
            return Ok(true);
        }
        ParsedLine::ApproveDecision { decision_id, scope } => {
            match approve_decision(api, &app.tenant, &decision_id, scope).await {
                Ok(()) => app.push_system(format!(
                    "Approval recorded for `{decision_id}` ({})",
                    approval_scope_label(scope)
                )),
                Err(error) => {
                    app.push_system(format!("Approval failed for `{decision_id}`: {error}"))
                }
            }
            return Ok(true);
        }
        ParsedLine::DenyDecision(decision_id) => {
            match deny_decision(api, &app.tenant, &decision_id).await {
                Ok(()) => app.push_system(format!("Denial recorded for `{decision_id}`.")),
                Err(error) => app.push_system(format!("Deny failed for `{decision_id}`: {error}")),
            }
            return Ok(true);
        }
        ParsedLine::ApprovePlan(plan_id) => {
            match approve_plan(api, &plan_id).await {
                Ok(()) => app.push_system(format!("Plan `{plan_id}` approved.")),
                Err(error) => {
                    app.push_system(format!("Plan approval failed for `{plan_id}`: {error}"))
                }
            }
            return Ok(true);
        }
        ParsedLine::RequestPlanChanges { plan_id, notes } => {
            match request_plan_changes(api, &plan_id, &notes).await {
                Ok(()) => app.push_system(format!("Requested changes on plan `{plan_id}`.")),
                Err(error) => {
                    app.push_system(format!("Request changes failed for `{plan_id}`: {error}"))
                }
            }
            return Ok(true);
        }
        ParsedLine::Message { content, command } => {
            app.push_user(content.clone());
            app.status = "thinking".to_string();
            let params = receive_message_params(
                &next_message_id(),
                &author_id(),
                &app.session_key,
                &content,
                &command,
            );
            api.dispatch_action(
                "Channels",
                &app.channel_entity_id,
                "Paw.Channel.ReceiveMessage",
                params,
            )
            .await
            .map_err(anyhow::Error::msg)?;
        }
    }

    Ok(true)
}

fn apply_event(app: &mut AppState, event: TuiEvent) {
    match event {
        TuiEvent::Reply { content } => {
            app.push_assistant(content);
            app.status = "idle".to_string();
        }
        TuiEvent::Status { text } => {
            if !text.trim().is_empty() {
                app.status = text;
            }
        }
        TuiEvent::System { text } => app.push_system(text),
    }
}

fn render<W: Write>(out: &mut W, app: &AppState) -> std::io::Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    let rows = rows.max(8);
    queue!(out, Hide, MoveTo(0, 0), Clear(ClearType::All))?;

    queue!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print("TemperPaw TUI"),
        ResetColor,
        SetAttribute(Attribute::Reset),
        Print(format!(
            "  {}  tenant={}  session={}  status={}",
            app.base_url, app.tenant, app.session_key, app.status
        ))
    )?;

    queue!(out, MoveTo(0, 1), Print("─".repeat(cols as usize)))?;

    let input_rows = 2;
    let footer_row = rows.saturating_sub(input_rows);
    let body_height = footer_row.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    for message in &app.messages {
        lines.extend(format_message(message, cols as usize));
    }
    let start = lines.len().saturating_sub(body_height);
    for (idx, line) in lines[start..].iter().enumerate() {
        queue!(
            out,
            MoveTo(0, 2 + idx as u16),
            Print(truncate(line, cols as usize))
        )?;
    }

    queue!(out, MoveTo(0, footer_row), Print("─".repeat(cols as usize)))?;
    queue!(
        out,
        MoveTo(0, footer_row + 1),
        SetForegroundColor(Color::DarkGrey),
        Print("Enter sends. Ctrl+C exits. "),
        ResetColor,
        Print("> "),
        Print(truncate(&app.input, cols.saturating_sub(3) as usize)),
        Show
    )?;
    out.flush()
}

fn format_message(message: &Message, width: usize) -> Vec<String> {
    let (prefix, color_marker) = match message.role {
        Role::User => ("you", ""),
        Role::Assistant => ("paw", ""),
        Role::System => ("system", ""),
    };
    let raw = format!("{prefix}: {}", message.content);
    wrap_line(&raw, width.max(20))
        .into_iter()
        .map(|line| {
            if color_marker.is_empty() {
                line
            } else {
                format!("{color_marker}{line}")
            }
        })
        .collect()
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut out = Vec::new();
    for source_line in text.lines() {
        let mut line = source_line.to_string();
        while line.chars().count() > width {
            let split_at = line
                .char_indices()
                .nth(width)
                .map(|(idx, _)| idx)
                .unwrap_or(line.len());
            out.push(line[..split_at].to_string());
            line = format!("  {}", &line[split_at..]);
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width.saturating_sub(1)).collect()
}

enum ParsedLine {
    Message {
        content: String,
        command: String,
    },
    Help,
    Status,
    Session(String),
    ApproveDecision {
        decision_id: String,
        scope: ApprovalScope,
    },
    DenyDecision(String),
    ApprovePlan(String),
    RequestPlanChanges {
        plan_id: String,
        notes: String,
    },
    Exit,
    Unknown(String),
}

fn parse_command(line: &str) -> ParsedLine {
    if !line.starts_with('/') {
        return ParsedLine::Message {
            content: line.to_string(),
            command: String::new(),
        };
    }

    let mut parts = line.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("").trim_start_matches('/');
    let rest = parts.next().unwrap_or("").trim();
    match command {
        "help" => ParsedLine::Help,
        "status" => ParsedLine::Status,
        "exit" | "quit" => ParsedLine::Exit,
        "plan" | "execute" => ParsedLine::Message {
            content: rest.to_string(),
            command: command.to_string(),
        },
        "reset" | "new" => ParsedLine::Message {
            content: rest.to_string(),
            command: "reset".to_string(),
        },
        "session" if !rest.is_empty() => ParsedLine::Session(rest.to_string()),
        "session" => ParsedLine::Unknown("/session requires a key".to_string()),
        "approve" | "approve-always" | "allow-always" if !rest.is_empty() => {
            ParsedLine::ApproveDecision {
                decision_id: rest.to_string(),
                scope: ApprovalScope::AlwaysForAgent,
            }
        }
        "approve" | "approve-always" | "allow-always" => {
            ParsedLine::Unknown(format!("/{command} requires a decision id"))
        }
        "approve-session" | "allow-session" if !rest.is_empty() => ParsedLine::ApproveDecision {
            decision_id: rest.to_string(),
            scope: ApprovalScope::Session,
        },
        "approve-session" | "allow-session" => {
            ParsedLine::Unknown(format!("/{command} requires a decision id"))
        }
        "approve-once" | "allow-once" if !rest.is_empty() => ParsedLine::ApproveDecision {
            decision_id: rest.to_string(),
            scope: ApprovalScope::Once,
        },
        "approve-once" | "allow-once" => {
            ParsedLine::Unknown(format!("/{command} requires a decision id"))
        }
        "deny" if !rest.is_empty() => ParsedLine::DenyDecision(rest.to_string()),
        "deny" => ParsedLine::Unknown("/deny requires a decision id".to_string()),
        "plan-approve" | "approve-plan" if !rest.is_empty() => {
            ParsedLine::ApprovePlan(rest.to_string())
        }
        "plan-approve" | "approve-plan" => {
            ParsedLine::Unknown(format!("/{command} requires a plan id"))
        }
        "request-changes" | "plan-request-changes" => {
            let mut args = rest.splitn(2, char::is_whitespace);
            let plan_id = args.next().unwrap_or("").trim();
            if plan_id.is_empty() {
                ParsedLine::Unknown(format!("/{command} requires a plan id"))
            } else {
                ParsedLine::RequestPlanChanges {
                    plan_id: plan_id.to_string(),
                    notes: args.next().unwrap_or("").trim().to_string(),
                }
            }
        }
        other => ParsedLine::Unknown(format!("/{other}")),
    }
}

async fn approve_decision(
    api: &PawApiClient,
    tenant: &str,
    decision_id: &str,
    scope: ApprovalScope,
) -> Result<(), String> {
    validate_key("decision", decision_id).map_err(|error| error.to_string())?;
    let url = format!(
        "{}/api/tenants/{tenant}/decisions/{decision_id}/approve",
        api.config().base_url
    );
    let decision = fetch_pending_decision(api, &api.config().base_url, tenant, decision_id)
        .await
        .ok()
        .flatten();
    let body = approval_body_for_scope(scope, decision.as_ref(), format!("cli:{}", author_id()))?;
    api.raw_post(&url, body).await?;
    Ok(())
}

fn approval_scope_label(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::AlwaysForAgent => "allow always",
        ApprovalScope::Session => "allow session",
        ApprovalScope::Once => "allow once",
    }
}

async fn deny_decision(api: &PawApiClient, tenant: &str, decision_id: &str) -> Result<(), String> {
    validate_key("decision", decision_id).map_err(|error| error.to_string())?;
    let url = format!(
        "{}/api/tenants/{tenant}/decisions/{decision_id}/deny",
        api.config().base_url
    );
    api.raw_post(
        &url,
        json!({ "decided_by": format!("cli:{}", author_id()) }),
    )
    .await?;
    Ok(())
}

async fn approve_plan(api: &PawApiClient, plan_id: &str) -> Result<(), String> {
    validate_key("plan", plan_id).map_err(|error| error.to_string())?;
    api.dispatch_action("Plans", plan_id, "TemperPaw.Approve", json!({}))
        .await?;
    Ok(())
}

async fn request_plan_changes(
    api: &PawApiClient,
    plan_id: &str,
    notes: &str,
) -> Result<(), String> {
    validate_key("plan", plan_id).map_err(|error| error.to_string())?;
    let review_notes = if notes.trim().is_empty() {
        format!(
            "Changes requested by cli:{}. Review the plan, revise it, and resubmit for approval.",
            author_id()
        )
    } else {
        notes.trim().to_string()
    };
    api.dispatch_action(
        "Plans",
        plan_id,
        "TemperPaw.RequestChanges",
        json!({ "review_notes": review_notes }),
    )
    .await?;
    Ok(())
}

fn should_exit_immediately(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn normalize_url(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

fn validate_key(label: &str, value: &str) -> anyhow::Result<()> {
    let valid = !value.trim().is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        anyhow::bail!("{label} may only contain letters, numbers, '-', '_', '.', or ':'")
    }
}

fn next_message_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("cli-{millis}")
}

fn author_id() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "terminal".to_string())
}

impl AppState {
    fn push_user(&mut self, content: String) {
        self.push(Role::User, content);
    }

    fn push_assistant(&mut self, content: String) {
        self.push(Role::Assistant, content);
    }

    fn push_system(&mut self, content: impl Into<String>) {
        self.push(Role::System, content.into());
    }

    fn push(&mut self, role: Role, content: String) {
        if self.messages.len() >= MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(Message { role, content });
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reset_as_receive_message_command() {
        match parse_command("/reset") {
            ParsedLine::Message { content, command } => {
                assert_eq!(content, "");
                assert_eq!(command, "reset");
            }
            _ => panic!("expected reset message command"),
        }
    }

    #[test]
    fn parses_session_switch() {
        match parse_command("/session bugfix") {
            ParsedLine::Session(session) => assert_eq!(session, "bugfix"),
            _ => panic!("expected session switch"),
        }
    }

    #[test]
    fn parses_plan_and_execute_as_transport_commands() {
        match parse_command("/plan ship the tui") {
            ParsedLine::Message { content, command } => {
                assert_eq!(content, "ship the tui");
                assert_eq!(command, "plan");
            }
            _ => panic!("expected plan message command"),
        }

        match parse_command("/execute ship the tui") {
            ParsedLine::Message { content, command } => {
                assert_eq!(content, "ship the tui");
                assert_eq!(command, "execute");
            }
            _ => panic!("expected execute message command"),
        }
    }

    #[test]
    fn parses_review_actions() {
        match parse_command("/approve PD-123") {
            ParsedLine::ApproveDecision { decision_id, scope } => {
                assert_eq!(decision_id, "PD-123");
                assert_eq!(scope, ApprovalScope::AlwaysForAgent);
            }
            _ => panic!("expected approve decision command"),
        }

        match parse_command("/approve-session PD-124") {
            ParsedLine::ApproveDecision { decision_id, scope } => {
                assert_eq!(decision_id, "PD-124");
                assert_eq!(scope, ApprovalScope::Session);
            }
            _ => panic!("expected session approval command"),
        }

        match parse_command("/approve-once PD-125") {
            ParsedLine::ApproveDecision { decision_id, scope } => {
                assert_eq!(decision_id, "PD-125");
                assert_eq!(scope, ApprovalScope::Once);
            }
            _ => panic!("expected once approval command"),
        }

        match parse_command("/deny PD-456") {
            ParsedLine::DenyDecision(decision_id) => assert_eq!(decision_id, "PD-456"),
            _ => panic!("expected deny decision command"),
        }

        match parse_command("/plan-approve pl-1") {
            ParsedLine::ApprovePlan(plan_id) => assert_eq!(plan_id, "pl-1"),
            _ => panic!("expected approve plan command"),
        }

        match parse_command("/request-changes pl-2 use smaller steps") {
            ParsedLine::RequestPlanChanges { plan_id, notes } => {
                assert_eq!(plan_id, "pl-2");
                assert_eq!(notes, "use smaller steps");
            }
            _ => panic!("expected request changes command"),
        }
    }
}
