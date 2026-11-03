//! Apply supported Postgres settings to one registered Supabase project.
use dsf_resource_common::{
    Error, Evidence, Host, Invocation, Receipt, ResourceAction, Runtime, Verification,
    VerifiedValue, required,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub project_ref: String,
    pub token_secret: String,
}

/// Numeric units avoid ambiguous Postgres configuration strings at the boundary.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub statement_timeout_ms: Option<u32>,
    pub work_mem_kib: Option<u32>,
    pub max_connections: Option<u32>,
    pub log_connections: Option<bool>,
    pub log_disconnections: Option<bool>,
    pub log_lock_waits: Option<bool>,
}

pub struct ApplyConfiguration;

impl Target {
    fn url(&self) -> Result<String, Error> {
        if self.project_ref.len() != 20
            || !self
                .project_ref
                .bytes()
                .all(|byte| byte.is_ascii_lowercase())
        {
            return Err(Error::Binding("invalid Supabase project reference"));
        }
        Ok(format!(
            "https://api.supabase.com/v1/projects/{}/config/database/postgres",
            self.project_ref
        ))
    }
}

impl Change {
    fn body(&self) -> Value {
        let mut body = serde_json::Map::new();
        for (key, value) in [
            ("statement_timeout", self.statement_timeout_ms),
            ("work_mem", self.work_mem_kib),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), value.to_string().into());
            }
        }
        if let Some(value) = self.max_connections {
            body.insert("max_connections".into(), value.into());
        }
        for (key, value) in [
            ("log_connections", self.log_connections),
            ("log_disconnections", self.log_disconnections),
            ("log_lock_waits", self.log_lock_waits),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), value.into());
            }
        }
        Value::Object(body)
    }

    fn matches(&self, actual: &Value) -> Result<bool, Error> {
        let number = |name: &str, units: &[(&str, u64)]| {
            let value = actual.get(name)?;
            if let Some(value) = value.as_u64() {
                return Some(value);
            }
            let raw = value.as_str()?.trim();
            for (suffix, scale) in units {
                if let Some(digits) = raw.strip_suffix(suffix) {
                    return digits.trim().parse::<u64>().ok()?.checked_mul(*scale);
                }
            }
            raw.parse().ok()
        };
        let mut same = true;
        for (key, expected, units) in [
            (
                "statement_timeout",
                self.statement_timeout_ms,
                &[("ms", 1), ("min", 60_000), ("s", 1000), ("h", 3_600_000)][..],
            ),
            (
                "work_mem",
                self.work_mem_kib,
                &[("kB", 1), ("MB", 1024), ("GB", 1_048_576)][..],
            ),
            ("max_connections", self.max_connections, &[][..]),
        ] {
            if let Some(expected) = expected {
                let actual =
                    number(key, units).ok_or(Error::Response("Supabase numeric setting"))?;
                same &= actual == u64::from(expected);
            }
        }
        for (key, expected) in [
            ("log_connections", self.log_connections),
            ("log_disconnections", self.log_disconnections),
            ("log_lock_waits", self.log_lock_waits),
        ] {
            if let Some(expected) = expected {
                let actual = actual
                    .get(key)
                    .and_then(Value::as_bool)
                    .ok_or(Error::Response("Supabase boolean setting"))?;
                same &= actual == expected;
            }
        }
        Ok(same)
    }
}

impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = Change;
    const ENTITY_TYPE: &'static str = "DsfSupabaseProject";
    const ENTITY_SET: &'static str = "DsfSupabaseProjects";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;

    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.url()?;
        if required(resource, "project_ref")? != target.project_ref {
            return Err(Error::Binding("Supabase project changed"));
        }
        Ok(())
    }

    fn validate_change(_: &Target, change: &Change, _: &Invocation) -> Result<(), Error> {
        if change.body().as_object().is_none_or(|body| body.is_empty())
            || change
                .statement_timeout_ms
                .is_some_and(|value| value > i32::MAX as u32)
            || change
                .work_mem_kib
                .is_some_and(|value| !(64..=i32::MAX as u32).contains(&value))
            || change
                .max_connections
                .is_some_and(|value| !(1..=i32::MAX as u32).contains(&value))
        {
            return Err(Error::Binding("invalid or empty Postgres configuration"));
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
        let response =
            runtime.bearer_json(&target.token_secret, "PUT", url.clone(), change.body())?;
        if !response.is_object() {
            return Err(Error::Response("Supabase postgres config"));
        }
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
        let actual = runtime.bearer_json(&target.token_secret, "GET", url.clone(), json!({}))?;
        if !actual.is_object() {
            return Err(Error::Response("Supabase postgres config"));
        }
        if !change.matches(&actual)? {
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
