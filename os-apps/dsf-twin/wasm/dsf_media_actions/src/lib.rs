//! Selected DSF media recovery with durable receipts and attempt ownership.
use dsf_resource_common::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;
mod links;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub application_id: String,
    pub environment_id: String,
    pub api_resource_id: String,
    pub bucket_resource_id: String,
    pub token_secret: String,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    CoverImage,
    Thumbnail,
    Video,
}
impl MediaType {
    fn name(self) -> &'static str {
        match self {
            Self::CoverImage => "cover_image",
            Self::Thumbnail => "thumbnail",
            Self::Video => "video",
        }
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    World,
    Story,
}
impl TargetType {
    fn name(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Story => "story",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub id: Uuid,
    pub target_type: TargetType,
    pub target_id: Uuid,
    pub media_type: MediaType,
    pub max_cost_cents: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub generations: Vec<Generation>,
    pub max_cost_cents: u64,
    pub cost_authority_ref: String,
}
pub struct RetrySelected;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CostAuthority {
    max_cost_cents: u32,
    agent_auth: AgentAuth,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AgentAuth {
    SubscriptionsOnly,
}

/// A provider UUID binds the entire resource command identity, including sequence.
pub fn operation_id(invocation: &Invocation) -> Uuid {
    let identity = serde_json::to_vec(&(
        &invocation.resource_id,
        &invocation.operation_key,
        invocation.sequence,
    ))
    .expect("serializable identity");
    let digest = Sha256::digest(identity);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
fn receipt_url(i: &Invocation) -> String {
    format!(
        "{DSF_API}/api/media/recovery-operations/{}",
        operation_id(i)
    )
}
fn status_url(id: Uuid) -> String {
    format!("{DSF_API}/api/media/{id}/status")
}
fn receipt(i: &Invocation) -> Receipt {
    Receipt {
        execution_id: operation_id(i).to_string(),
        evidence_ref: receipt_url(i),
    }
}
fn selected(c: &Change) -> Vec<Uuid> {
    c.generations.iter().map(|g| g.id).collect()
}
fn validate_selection(c: &Change, i: &Invocation) -> Result<(), Error> {
    if c.generations.is_empty() || c.generations.len() > 20 || c.max_cost_cents == 0 {
        return Err(Error::Binding(
            "media repair needs 1 to 20 jobs and a cost ceiling",
        ));
    }
    let ids = selected(c);
    let mut unique = BTreeSet::new();
    let mut cost = 0u64;
    for job in &c.generations {
        if job.id.is_nil()
            || job.target_id.is_nil()
            || !unique.insert(job.id)
            || job.max_cost_cents == 0
        {
            return Err(Error::Binding(
                "media jobs must be unique with positive ceilings",
            ));
        }
        cost = cost
            .checked_add(job.max_cost_cents)
            .ok_or(Error::Binding("media cost ceiling overflow"))?;
    }
    if cost > c.max_cost_cents {
        return Err(Error::Binding(
            "selected job ceilings exceed operation ceiling",
        ));
    }
    let row_ids: Vec<Uuid> =
        serde_json::from_value(decoded(&i.resource, "selected_generation_ids")?)
            .map_err(|_| Error::Binding("invalid selected job IDs"))?;
    if row_ids != ids
        || required(&i.resource, "cost_authority_ref")? != c.cost_authority_ref
        || c.cost_authority_ref.is_empty()
    {
        return Err(Error::Binding(
            "media selection or cost authority differs from accepted action",
        ));
    }
    if !full_sha(&i.revision) {
        return Err(Error::Binding("media repair requires exact API revision"));
    }
    Ok(())
}
fn status(r: &mut Runtime<impl Host>, t: &Target, g: &Generation) -> Result<Value, Error> {
    let row = r.bearer_json(&t.token_secret, "GET", status_url(g.id), Value::Null)?;
    let id = required(&row, "generation_id")?
        .parse::<Uuid>()
        .map_err(|_| Error::Response("media generation ID"))?;
    let target_id = required(&row, "target_id")?
        .parse::<Uuid>()
        .map_err(|_| Error::Response("media target ID"))?;
    if id != g.id
        || target_id != g.target_id
        || required(&row, "target_type")? != g.target_type.name()
        || required(&row, "media_type")? != g.media_type.name()
    {
        return Err(Error::Binding("media generation target or type differs"));
    }
    Ok(row)
}
fn preflight(
    r: &mut Runtime<impl Host>,
    t: &Target,
    c: &Change,
    i: &Invocation,
) -> Result<(), Error> {
    let health = json_body(
        r.host.request(&Request {
            method: "GET",
            url: format!("{DSF_API}/api/health"),
            headers: vec![],
            body: String::new(),
        })?,
        "DSF health",
    )?;
    if required(&health, "status")? != "healthy" || required(&health, "git_sha")? != i.revision {
        return Err(Error::Binding("DSF API is not at the authorized revision"));
    }
    for generation in &c.generations {
        let row = status(r, t, generation)?;
        if row
            .get("attempt_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<Uuid>().ok())
            == Some(operation_id(i))
        {
            return Err(Error::Pending(
                "selected job retains this operation but its receipt is unavailable",
            ));
        }
        if !["failed", "generating"].contains(&required(&row, "status")?) {
            return Err(Error::Binding(
                "selected retry requires failed or stuck generation",
            ));
        }
        let estimate = match generation.media_type {
            MediaType::CoverImage | MediaType::Thumbnail => 2,
            MediaType::Video => {
                let seconds = row
                    .get("duration_seconds")
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite() && (5.0..=15.0).contains(n) && n.fract() == 0.0)
                    .ok_or(Error::Binding(
                        "video duration must be 5 to 15 whole seconds",
                    ))?;
                seconds as u64 * 5
            }
        };
        if estimate > generation.max_cost_cents {
            return Err(Error::Binding(
                "media provider price exceeds selected job ceiling",
            ));
        }
    }
    Ok(())
}
struct Recovery {
    claimed: BTreeSet<Uuid>,
}
fn recovery_response(
    row: &Value,
    c: &Change,
    i: &Invocation,
    stored: bool,
) -> Result<Recovery, Error> {
    if required(row, "operation_id")? != operation_id(i).to_string() {
        return Err(Error::Binding("media receipt belongs to another operation"));
    }
    let replayed = row
        .get("replayed")
        .and_then(Value::as_bool)
        .ok_or(Error::Response("media receipt replay flag"))?;
    if stored && replayed {
        return Err(Error::Response(
            "stored media receipt is not the original result",
        ));
    }
    let items = row
        .get("generations")
        .and_then(Value::as_array)
        .ok_or(Error::Response("media receipt outcomes"))?;
    if items.len() != c.generations.len() {
        return Err(Error::Binding("media receipt selection differs"));
    }
    let expected: BTreeSet<_> = selected(c).into_iter().collect();
    let mut returned = BTreeSet::new();
    let mut claimed = BTreeSet::new();
    for item in items {
        let id = required(item, "generation_id")?
            .parse::<Uuid>()
            .map_err(|_| Error::Response("media receipt generation ID"))?;
        if !expected.contains(&id) || !returned.insert(id) {
            return Err(Error::Binding("media receipt selection differs"));
        }
        match required(item, "outcome")? {
            "claimed" => {
                claimed.insert(id);
            }
            "ineligible" | "missing" => {}
            _ => return Err(Error::Response("media receipt outcome")),
        }
    }
    if row.get("queued").and_then(Value::as_u64) != Some(claimed.len() as u64) {
        return Err(Error::Response(
            "media receipt count differs from claimed jobs",
        ));
    }
    Ok(Recovery { claimed })
}
fn read_receipt(
    r: &mut Runtime<impl Host>,
    t: &Target,
    c: &Change,
    i: &Invocation,
) -> Result<Option<Recovery>, Error> {
    let row = match r.bearer_json(&t.token_secret, "GET", receipt_url(i), Value::Null) {
        Ok(row) => row,
        Err(Error::Http(404, _)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if required(&row, "operation_id")? != operation_id(i).to_string()
        || required(&row, "endpoint")? != "/api/media/retry-stuck"
    {
        return Err(Error::Binding(
            "media receipt belongs to another operation or endpoint",
        ));
    }
    let ids: Vec<Uuid> = serde_json::from_value(
        row.get("generation_ids")
            .cloned()
            .ok_or(Error::Response("media receipt selection"))?,
    )
    .map_err(|_| Error::Response("media receipt selection"))?;
    let expected: BTreeSet<_> = selected(c).into_iter().collect();
    let actual: BTreeSet<_> = ids.iter().copied().collect();
    if ids.len() != expected.len() || actual != expected {
        return Err(Error::Binding("media receipt selection differs"));
    }
    recovery_response(
        row.get("response")
            .ok_or(Error::Response("media receipt response"))?,
        c,
        i,
        true,
    )
    .map(Some)
}
fn settled(
    r: &mut Runtime<impl Host>,
    t: &Target,
    c: &Change,
    i: &Invocation,
    recovery: &Recovery,
) -> Result<Vec<(usize, Value)>, Error> {
    let mut states = Vec::new();
    let attempt = operation_id(i);
    for (index, generation) in c
        .generations
        .iter()
        .enumerate()
        .filter(|(_, g)| recovery.claimed.contains(&g.id))
    {
        let row = status(r, t, generation)?;
        if row
            .get("attempt_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<Uuid>().ok())
            != Some(attempt)
        {
            return Err(Error::Pending("media job is owned by another attempt"));
        }
        if !["completed", "failed"].contains(&required(&row, "status")?) {
            return Err(Error::Pending("claimed media work is still active"));
        }
        states.push((index, row));
    }
    Ok(states)
}
fn artifact_url(raw: &str, generation: &Generation, attempt: Uuid) -> Result<(), Error> {
    let parsed = url::Url::parse(raw).map_err(|_| Error::Binding("invalid media artifact URL"))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed
            .host_str()
            .is_some_and(|host| host == "media.deep-sci-fi.world" || host.ends_with(".r2.dev"))
    {
        return Err(Error::Binding("media artifact host is not permitted"));
    }
    let extension = if generation.media_type == MediaType::Video {
        "mp4"
    } else {
        "png"
    };
    let expected = format!(
        "/media/{}/{}/{}/{}/{}.{}",
        generation.target_type.name(),
        generation.target_id,
        generation.media_type.name(),
        generation.id,
        attempt,
        extension
    );
    if parsed.path() != expected {
        return Err(Error::Binding(
            "media object belongs to another generation or attempt",
        ));
    }
    Ok(())
}
impl ResourceAction for RetrySelected {
    type Target = Target;
    type Change = Change;
    const ENTITY_TYPE: &'static str = "DsfMediaPipeline";
    const ENTITY_SET: &'static str = "DsfMediaPipelines";
    const ACTION: &'static str = "RetrySelected";
    const RESULT: VerifiedValue = VerifiedValue::Revision;
    fn validate_target(t: &Target, row: &Value) -> Result<(), Error> {
        if t.environment_id != "production" {
            return Err(Error::Binding("this DSF media API is production only"));
        }
        for (name, wanted) in [
            ("application_id", &t.application_id),
            ("environment_id", &t.environment_id),
            ("api_resource_id", &t.api_resource_id),
            ("bucket_resource_id", &t.bucket_resource_id),
        ] {
            identifier(wanted)?;
            if required(row, name)? != wanted {
                return Err(Error::Binding("media pipeline target differs"));
            }
        }
        identifier(&t.token_secret)?;
        Ok(())
    }
    fn validate_change(_: &Target, c: &Change, i: &Invocation) -> Result<(), Error> {
        validate_selection(c, i)
    }
    fn validate_authority(
        runtime: &mut Runtime<impl Host>,
        config: &ResourceConfig<Target>,
        change: &Change,
        invocation: &Invocation,
    ) -> Result<(), Error> {
        if !config.required_ask_ids.contains(&change.cost_authority_ref) {
            return Err(Error::Binding(
                "media cost authority is not a required linked Ask",
            ));
        }
        let ask = runtime.row("Asks", &change.cost_authority_ref)?;
        if required(&ask, "effort_id")? != invocation.effort_id {
            return Err(Error::Binding(
                "media cost authority belongs to another Effort",
            ));
        }
        if required(&ask, "status")? != "Answered" {
            return Err(Error::Blocked(change.cost_authority_ref.clone()));
        }
        required(&ask, "who")?;
        let authority: CostAuthority = serde_json::from_str(required(&ask, "chose")?)
            .map_err(|_| Error::Binding("media cost authority must contain a numeric ceiling and subscriptions_only agent auth"))?;
        let AgentAuth::SubscriptionsOnly = authority.agent_auth;
        let total = change
            .generations
            .iter()
            .try_fold(0u64, |total, job| total.checked_add(job.max_cost_cents))
            .ok_or(Error::Binding("media cost overflow"))?;
        if authority.max_cost_cents == 0
            || total > u64::from(authority.max_cost_cents).min(change.max_cost_cents)
        {
            return Err(Error::Binding(
                "selected media ceilings exceed the answered authority",
            ));
        }
        Ok(())
    }
    fn execute(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &Change,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        if read_receipt(r, t, c, i)?.is_some() {
            return Ok(receipt(i));
        }
        preflight(r, t, c, i)?;
        links::verify(r, t)?;
        // The DSF endpoint serializes this UUID and stores the selected IDs and
        // original response in the same transaction as the attempt claims.
        let row = r.bearer_json(
            &t.token_secret,
            "POST",
            format!("{DSF_API}/api/media/retry-stuck"),
            json!({"operation_id":operation_id(i),"generation_ids":selected(c)}),
        )?;
        recovery_response(&row, c, i, false)?;
        Ok(receipt(i))
    }
    fn observe(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &Change,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        if read_receipt(r, t, c, i)?.is_some() {
            Ok(receipt(i))
        } else {
            Err(Error::Absent(receipt_url(i)))
        }
    }
    fn verify(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &Change,
        i: &Invocation,
        v: &Verification,
    ) -> Result<Evidence, Error> {
        if !matches!(v.flow, Flow::Media {}) {
            return Err(Error::Binding(
                "media repair requires selected media verification",
            ));
        }
        let recovery = read_receipt(r, t, c, i)?
            .ok_or(Error::Pending("media recovery receipt is not visible"))?;
        let states = settled(r, t, c, i, &recovery)?;
        // Do not release the pipeline while any successfully claimed work may
        // still incur cost, even when another selection was refused or failed.
        if recovery.claimed.len() != c.generations.len() {
            return Err(Error::ProviderFailed(
                "not every selected media job was claimed",
            ));
        }
        if states
            .iter()
            .any(|(_, row)| row.get("status").and_then(Value::as_str) == Some("failed"))
        {
            return Err(Error::ProviderFailed("selected media generation failed"));
        }
        let mut total_cost = 0.0;
        for (index, row) in states {
            let generation = &c.generations[index];
            let url = required(&row, "media_url")?;
            artifact_url(url, generation, operation_id(i))?;
            let response = r.host.request(&Request {
                method: "HEAD",
                url: url.into(),
                headers: vec![],
                body: String::new(),
            })?;
            if response.status != 200 {
                return Err(Error::Pending("completed media artifact is not accessible"));
            }
            let cost = row
                .get("cost_usd")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or(Error::Pending("completed media cost is not recorded"))?
                * 100.0;
            if cost > generation.max_cost_cents as f64 + 0.000001 {
                return Err(Error::ProviderFailed("media job exceeded its cost ceiling"));
            }
            total_cost += cost;
        }
        if total_cost > c.max_cost_cents as f64 + 0.000001 {
            return Err(Error::ProviderFailed(
                "media repair exceeded its cost ceiling",
            ));
        }
        let (_, telemetry_ref, observed_revision) =
            verify_product(r, v, i, DSF_API, Some(&i.revision))?;
        Ok(Evidence {
            provider_ref: receipt_url(i),
            flow_ref: receipt_url(i),
            telemetry_ref,
            observed_revision,
            observed_configuration: i.configuration.clone(),
        })
    }
}
#[cfg(test)]
mod tests;
