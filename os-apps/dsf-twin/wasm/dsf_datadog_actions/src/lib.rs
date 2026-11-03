//! Edit one registered Datadog monitor using its existing organization credentials.
use dsf_resource_common::{
    Error, Evidence, Host, Invocation, Receipt, Request, ResourceAction, Runtime, Verification,
    VerifiedValue, json_body, required,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub site: String,
    pub organization_id: String,
    pub monitor_id: u64,
    pub api_key_secret: String,
    pub app_key_secret: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_no_data: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_data_timeframe: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_full_window: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_tags: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation_delay: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_group_delay: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Thresholds>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critical_recovery: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_recovery: Option<f64>,
}
pub struct ApplyConfiguration;
impl Target {
    fn url(&self) -> Result<String, Error> {
        dsf_resource_common::datadog_site(&self.site)?;
        if self.monitor_id == 0 {
            return Err(Error::Binding("invalid Datadog monitor ID"));
        }
        Ok(format!(
            "https://api.{}/api/v1/monitor/{}",
            self.site, self.monitor_id
        ))
    }
    fn call(
        &self,
        runtime: &mut Runtime<impl Host>,
        method: &'static str,
        body: Value,
    ) -> Result<Value, Error> {
        let api_key = runtime.credential(&self.api_key_secret)?;
        let app_key = runtime.credential(&self.app_key_secret)?;
        let value = json_body(
            runtime.host.request(&Request {
                method,
                url: self.url()?,
                headers: vec![
                    ("DD-API-KEY".into(), api_key),
                    ("DD-APPLICATION-KEY".into(), app_key),
                    ("content-type".into(), "application/json".into()),
                ],
                body: if method == "GET" {
                    String::new()
                } else {
                    body.to_string()
                },
            })?,
            "Datadog monitor",
        )?;
        if value.get("id").and_then(Value::as_u64) != Some(self.monitor_id) {
            return Err(Error::Response("Datadog monitor identity"));
        }
        Ok(value)
    }
}
fn same_patch(desired: &Value, actual: &Value) -> Result<bool, Error> {
    if let Some(object) = desired.as_object() {
        let mut same = true;
        for (key, value) in object {
            let observed = actual
                .get(key)
                .ok_or(Error::Response("Datadog monitor fields"))?;
            same &= same_patch(value, observed)?;
        }
        return Ok(same);
    }
    if let Some(expected) = desired.as_array() {
        let observed = actual
            .as_array()
            .ok_or(Error::Response("Datadog monitor tags"))?;
        if !observed.iter().all(Value::is_string) {
            return Err(Error::Response("Datadog monitor tags"));
        }
        return Ok(expected
            .iter()
            .map(Value::to_string)
            .collect::<BTreeSet<_>>()
            == observed
                .iter()
                .map(Value::to_string)
                .collect::<BTreeSet<_>>());
    }
    if let Some(expected) = desired.as_f64() {
        let observed = actual
            .as_f64()
            .ok_or(Error::Response("Datadog monitor numeric option"))?;
        return Ok(expected == observed);
    }
    if desired.is_boolean() != actual.is_boolean() || desired.is_string() != actual.is_string() {
        return Err(Error::Response("Datadog monitor field type"));
    }
    Ok(desired == actual)
}
impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = Change;
    const ENTITY_TYPE: &'static str = "DsfDatadogMonitor";
    const ENTITY_SET: &'static str = "DsfDatadogMonitors";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.url()?;
        if required(resource, "site")? != target.site
            || required(resource, "organization_id")? != target.organization_id
            || required(resource, "monitor_id")? != target.monitor_id.to_string()
        {
            return Err(Error::Binding("Datadog monitor binding changed"));
        }
        Ok(())
    }
    fn validate_change(_: &Target, change: &Change, _: &Invocation) -> Result<(), Error> {
        let body =
            serde_json::to_value(change).map_err(|_| Error::Binding("invalid monitor values"))?;
        if body.as_object().is_none_or(|object| object.is_empty())
            || change
                .name
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 500)
            || change
                .query
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 8192)
            || change
                .message
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
            || change
                .priority
                .is_some_and(|value| !(1..=5).contains(&value))
            || change.tags.as_ref().is_some_and(|tags| {
                tags.len() > 100
                    || tags.iter().any(|tag| tag.is_empty() || tag.len() > 256)
                    || tags.iter().collect::<BTreeSet<_>>().len() != tags.len()
            })
        {
            return Err(Error::Binding("invalid or empty monitor configuration"));
        }
        if let Some(options) = &change.options {
            if body["options"]
                .as_object()
                .is_none_or(|value| value.is_empty())
                || options
                    .no_data_timeframe
                    .is_some_and(|value| value == 0 || value > 10080)
                || options.evaluation_delay.is_some_and(|value| value > 86400)
                || options.new_group_delay.is_some_and(|value| value > 86400)
            {
                return Err(Error::Binding("invalid monitor options"));
            }
            if let Some(thresholds) = &options.thresholds {
                let values = [
                    thresholds.critical,
                    thresholds.warning,
                    thresholds.critical_recovery,
                    thresholds.warning_recovery,
                ];
                if values.iter().all(Option::is_none)
                    || values.iter().flatten().any(|value| !value.is_finite())
                {
                    return Err(Error::Binding("invalid monitor thresholds"));
                }
            }
        }
        Ok(())
    }
    fn execute(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Change,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        Self::validate_target(target, &invocation.resource)?;
        Self::validate_change(target, change, invocation)?;
        target.call(
            runtime,
            "PUT",
            serde_json::to_value(change).map_err(|_| Error::Binding("invalid monitor values"))?,
        )?;
        let url = target.url()?;
        Ok(Receipt {
            execution_id: url.clone(),
            evidence_ref: url,
        })
    }
    fn observe(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Change,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        Self::validate_target(target, &invocation.resource)?;
        Self::validate_change(target, change, invocation)?;
        let actual = target.call(runtime, "GET", json!({}))?;
        let desired =
            serde_json::to_value(change).map_err(|_| Error::Binding("invalid monitor values"))?;
        let url = target.url()?;
        if !same_patch(&desired, &actual)? {
            return Err(Error::Absent(url));
        }
        Ok(Receipt {
            execution_id: url.clone(),
            evidence_ref: url,
        })
    }
    fn verify(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Change,
        invocation: &Invocation,
        verification: &Verification,
    ) -> Result<Evidence, Error> {
        let receipt = Self::observe(runtime, target, change, invocation)?;
        dsf_resource_common::verify_configuration(
            runtime,
            verification,
            invocation,
            receipt.evidence_ref,
        )
    }
}
#[cfg(test)]
mod tests;
