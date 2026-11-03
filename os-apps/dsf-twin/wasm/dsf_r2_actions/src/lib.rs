//! Replace one registered R2 bucket's CORS policy; never mutate its objects.
use dsf_resource_common::{
    Error, Evidence, Host, Invocation, Receipt, ResourceAction, Runtime, Verification,
    VerifiedValue, required,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub account_id: String,
    pub bucket_name: String,
    pub token_secret: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub rules: Vec<Rule>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Rule {
    pub allowed: Allowed,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expose_headers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u32>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allowed {
    pub methods: Vec<Method>,
    pub origins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Put,
    Post,
    Delete,
    Head,
}
pub struct ApplyConfiguration;

impl Target {
    fn url(&self) -> Result<String, Error> {
        if self.account_id.len() != 32
            || !self.account_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !(3..=63).contains(&self.bucket_name.len())
            || !self
                .bucket_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.bucket_name.starts_with('-')
            || self.bucket_name.ends_with('-')
        {
            return Err(Error::Binding("invalid R2 account or bucket"));
        }
        Ok(format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}/cors",
            self.account_id, self.bucket_name
        ))
    }
}
fn strings(values: &[String], maximum: usize) -> bool {
    values.len() <= maximum
        && values
            .iter()
            .all(|value| !value.is_empty() && value.len() <= 2048 && !value.contains(['\r', '\n']))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
fn origin(value: &str) -> bool {
    if value == "*" {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/"
        && !value.ends_with('/')
}
fn same_strings(first: Option<&Vec<String>>, second: Option<&Vec<String>>) -> bool {
    first.into_iter().flatten().collect::<BTreeSet<_>>()
        == second.into_iter().flatten().collect::<BTreeSet<_>>()
}
impl Rule {
    fn matches(&self, actual: &Self) -> bool {
        self.id
            .as_ref()
            .is_none_or(|id| actual.id.as_ref() == Some(id))
            && self.allowed.methods.iter().collect::<BTreeSet<_>>()
                == actual.allowed.methods.iter().collect::<BTreeSet<_>>()
            && same_strings(Some(&self.allowed.origins), Some(&actual.allowed.origins))
            && same_strings(
                self.allowed.headers.as_ref(),
                actual.allowed.headers.as_ref(),
            )
            && same_strings(self.expose_headers.as_ref(), actual.expose_headers.as_ref())
            && self.max_age_seconds.unwrap_or(0) == actual.max_age_seconds.unwrap_or(0)
    }
}
fn envelope(value: Value) -> Result<Value, Error> {
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(Error::Response("R2 envelope"));
    }
    value
        .get("result")
        .cloned()
        .ok_or(Error::Response("R2 result"))
}
impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = Change;
    const ENTITY_TYPE: &'static str = "DsfCloudflareR2Bucket";
    const ENTITY_SET: &'static str = "DsfCloudflareR2Buckets";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.url()?;
        if required(resource, "account_id")? != target.account_id
            || required(resource, "bucket_name")? != target.bucket_name
        {
            return Err(Error::Binding("R2 bucket changed"));
        }
        Ok(())
    }
    fn validate_change(_: &Target, change: &Change, _: &Invocation) -> Result<(), Error> {
        if change.rules.len() > 100 {
            return Err(Error::Binding("too many CORS rules"));
        }
        for rule in &change.rules {
            if rule.allowed.methods.is_empty()
                || rule.allowed.methods.len() > 5
                || rule.allowed.methods.iter().collect::<BTreeSet<_>>().len()
                    != rule.allowed.methods.len()
                || rule.allowed.origins.is_empty()
                || !strings(&rule.allowed.origins, 100)
                || !rule.allowed.origins.iter().all(|value| origin(value))
                || rule
                    .allowed
                    .headers
                    .as_ref()
                    .is_some_and(|headers| !strings(headers, 100))
                || rule
                    .expose_headers
                    .as_ref()
                    .is_some_and(|headers| !strings(headers, 100))
                || rule.max_age_seconds.is_some_and(|age| age > 86400)
                || rule
                    .id
                    .as_ref()
                    .is_some_and(|id| id.is_empty() || id.len() > 255 || id.contains(['\r', '\n']))
            {
                return Err(Error::Binding("invalid CORS rule"));
            }
        }
        let keys: Vec<_> = change
            .rules
            .iter()
            .map(|rule| serde_json::to_string(rule).expect("typed rule serializes"))
            .collect();
        if keys.iter().collect::<BTreeSet<_>>().len() != keys.len() {
            return Err(Error::Binding("duplicate CORS rules"));
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
        let url = target.url()?;
        envelope(runtime.bearer_json(
            &target.token_secret,
            "PUT",
            url.clone(),
            serde_json::to_value(change).expect("typed CORS policy serializes"),
        )?)?;
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
        let url = target.url()?;
        let actual =
            envelope(runtime.bearer_json(&target.token_secret, "GET", url.clone(), json!({}))?)?;
        let actual: Change =
            serde_json::from_value(actual).map_err(|_| Error::Response("R2 CORS policy"))?;
        if actual.rules.len() > 100 {
            return Err(Error::Response("R2 CORS policy exceeds bounded rules"));
        }
        let mut remaining: Vec<_> = actual.rules.iter().collect();
        let matches = change.rules.iter().all(|rule| {
            if let Some(index) = remaining.iter().position(|observed| rule.matches(observed)) {
                remaining.remove(index);
                true
            } else {
                false
            }
        });
        if !matches || !remaining.is_empty() {
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
