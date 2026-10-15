//! Typed resource observations. IOA owns immutable evidence and CAS projection.
use chrono::{DateTime, SecondsFormat};
use dsf_resource_common::*;
pub use dsf_resource_common::{Error, Host, Request, Response, Runtime};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
mod providers;
pub use providers::{Datadog, Media, R2, Railway, Supabase, Vercel};
/// Explain fixed contract checks without copying provider bodies or field values.
pub fn failure_message(error: &Error) -> String {
    match error {
        Error::Binding(reason) => format!("resource observation binding failed: {reason}"),
        Error::Response(source) => {
            format!("resource observation received an invalid {source} response")
        }
        Error::Http(status, source) => {
            format!("resource observation received HTTP {status} from {source}")
        }
        Error::Transport => "resource observation transport failed".into(),
        _ => "resource observation required field or evidence failed".into(),
    }
}
#[cfg(target_arch = "wasm32")]
pub mod guest;

#[derive(PartialEq, Eq)]
pub enum Coverage {
    Measured,
    Absent,
    Inaccessible,
    Stale,
}
pub struct Facts {
    pub coverage: Coverage,
    pub outcome: String,
    pub revision: String,
    pub values: Value,
    pub sample_kind: &'static str,
    pub source_at_ms: Option<i64>,
}
impl Facts {
    fn measured(outcome: &str, revision: &str, values: Value) -> Self {
        Self {
            coverage: Coverage::Measured,
            outcome: outcome.into(),
            revision: revision.into(),
            values,
            sample_kind: "provider_resource",
            source_at_ms: None,
        }
    }
    fn unavailable(coverage: Coverage, outcome: &str, values: Value) -> Self {
        Self {
            coverage,
            outcome: outcome.into(),
            revision: String::new(),
            values,
            sample_kind: "provider_resource",
            source_at_ms: None,
        }
    }
}
pub trait Collector {
    type Binding: ResourceAction;
    const NOT_FOUND_IS_ABSENT: bool = true;
    fn source(target: &<Self::Binding as ResourceAction>::Target) -> String;
    fn query(target: &<Self::Binding as ResourceAction>::Target) -> String {
        Self::source(target)
    }
    fn read(
        runtime: &mut Runtime<impl Host>,
        target: &<Self::Binding as ResourceAction>::Target,
    ) -> Result<Facts, Error>;
}
fn timestamp(ms: i64) -> Result<String, Error> {
    DateTime::from_timestamp_millis(ms)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or(Error::Binding("invalid observation time"))
}
fn error_fact(error: Error, not_found_is_absent: bool) -> Facts {
    match error {
        Error::Http(404, _) if not_found_is_absent => Facts::unavailable(
            Coverage::Absent,
            "provider_not_found",
            json!({"http_status":404}),
        ),
        Error::Http(status, _) => Facts::unavailable(
            Coverage::Inaccessible,
            "provider_http_error",
            json!({"http_status":status}),
        ),
        Error::Transport => Facts::unavailable(
            Coverage::Inaccessible,
            "provider_unavailable",
            json!({"response_received":false}),
        ),
        _ => Facts::unavailable(
            Coverage::Inaccessible,
            "provider_shape_or_identity_error",
            json!({"valid_bound_response":false}),
        ),
    }
}
/// Captured sequence fences callbacks; the later IOA reaction preserves evidence
/// even if another observation wins the resource projection CAS.
pub fn collect<C: Collector>(
    runtime: &mut Runtime<impl Host>,
    resource_id: &str,
    captured: &Value,
) -> Result<Callback, Error> {
    identifier(resource_id)?;
    // Both counters read their declared initial of zero when the kernel has
    // never incremented them. A resource that has never been refreshed is
    // rejected below by its own message; a resource that has never been
    // observed legitimately fences its first callback against sequence zero.
    let sequence = counter(captured, "refresh_sequence")?;
    if required(captured, "status")? != "Refreshing" || sequence == 0 || runtime.now_ms <= 0 {
        return Err(Error::Binding("resource is not refreshing"));
    }
    // The HTTP projection is updated after this integration returns. The
    // committed invocation owns this read; callback guards fence later changes.
    let current = captured;
    let config_ref = required(&current, "config_ref")?;
    let raw = runtime.read("Files", config_ref, true)?;
    if raw.status != 200 || raw.body.len() > 32768 {
        return Err(Error::Response("resource configuration File"));
    }
    if format!("{:x}", Sha256::digest(raw.body.as_bytes())) != required(&current, "config_sha256")?
    {
        return Err(Error::Binding("resource configuration hash differs"));
    }
    let config: ResourceConfig<<C::Binding as ResourceAction>::Target> =
        serde_json::from_str(&raw.body)
            .map_err(|_| Error::Binding("invalid typed resource configuration"))?;
    if config.version != 3 || config.resource_id != resource_id {
        return Err(Error::Binding("configuration belongs to another resource"));
    }
    C::Binding::validate_target(&config.target, &current)?;
    let source = C::source(&config.target);
    let query = C::query(&config.target);
    let observed = C::read(runtime, &config.target)
        .unwrap_or_else(|error| error_fact(error, C::NOT_FOUND_IS_ABSENT));
    if observed.values.to_string().len() > 32768 || query.len() > 8192 {
        return Err(Error::Response("bounded observation evidence"));
    }
    if !observed.revision.is_empty() && !full_sha(&observed.revision) {
        return Err(Error::Response("provider revision"));
    }
    let observation_id = format!(
        "dsf-observation-{:x}",
        Sha256::digest(
            serde_json::to_vec(&(C::Binding::ENTITY_TYPE, resource_id, sequence))
                .expect("serializable observation identity")
        )
    );
    let mut params = json!({"expected_refresh_sequence":sequence,"error_message":"","collected_observation_id":observation_id,"collected_source_event_id":observation_id,"collected_query":query,"collected_window_start":timestamp(observed.source_at_ms.unwrap_or(runtime.now_ms).min(runtime.now_ms))?,"collected_window_end":timestamp(runtime.now_ms)?,"collected_sample_kind":observed.sample_kind,"collected_outcome":observed.outcome,"collected_summary":observed.values.to_string(),"collected_evidence_ref":source,"collected_observed_at_ms":runtime.now_ms,"collected_expected_resource_sequence":counter(&current,"observed_sequence")?});
    let action = match observed.coverage {
        Coverage::Measured => {
            params["collected_observed_configuration"] = json!(observed.values.to_string());
            params["collected_observed_revision"] = json!(observed.revision);
            "CollectionMeasured"
        }
        Coverage::Absent => "CollectionAbsent",
        Coverage::Inaccessible => "CollectionInaccessible",
        Coverage::Stale => "CollectionStale",
    };
    Ok(Callback {
        action: action.into(),
        params,
    })
}
fn picked(row: &Value, keys: &[&str]) -> Result<Value, Error> {
    let mut output = json!({});
    for key in keys {
        if let Some(value) = row.get(key) {
            if value.is_array()
                || value.is_object()
                || value.as_str().is_some_and(|s| s.len() > 4096)
            {
                return Err(Error::Response("provider scalar field"));
            }
            output[key] = value.clone();
        }
    }
    Ok(output)
}
fn revision(value: Option<&Value>) -> Result<String, Error> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(value) => {
            let sha = value.as_str().ok_or(Error::Response("provider commit"))?;
            if full_sha(sha) {
                Ok(sha.into())
            } else {
                Err(Error::Response("provider commit"))
            }
        }
    }
}
#[cfg(test)]
mod tests;
