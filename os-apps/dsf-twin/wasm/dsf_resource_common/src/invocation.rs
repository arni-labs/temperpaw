use crate::{Error, counter, field, identifier, required};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Invocation {
    pub resource_id: String,
    pub operation_key: String,
    pub sequence: u64,
    pub effort_id: String,
    pub revision: String,
    pub configuration: String,
    pub proof_id: String,
    pub config_ref: String,
    pub config_sha256: String,
    pub execution_id: Option<String>,
    pub execution_attempts: u64,
    pub resource: Value,
}

impl Invocation {
    pub fn require_stage<A: crate::ResourceAction>(&self, stage: &str) -> Result<(), Error> {
        if required(&self.resource, "status")? != format!("{}{stage}", A::ACTION) {
            return Err(Error::Binding(
                "invocation belongs to a different action phase",
            ));
        }
        Ok(())
    }

    pub fn parse(id: &str, resource: &Value) -> Result<Self, Error> {
        identifier(id)?;
        let text = |name: &str| required(resource, name).map(str::to_owned);
        let count = |name: &str| counter(resource, name);
        // Zero is the declared initial, so an unstarted operation must reach
        // its own message here rather than read as a missing field.
        let sequence = count("operation_sequence")?;
        if sequence == 0 {
            return Err(Error::Binding("operation has not started"));
        }
        let key = text("operation_key")?;
        identifier(&key)?;
        let digest = text("config_sha256")?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Binding("invalid configuration digest"));
        }
        Ok(Self {
            resource_id: id.into(),
            operation_key: key,
            sequence,
            effort_id: text("effort_id")?,
            revision: field(resource, "request_revision")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            configuration: text("request_configuration")?,
            proof_id: text("proof_ref")?,
            config_ref: text("config_ref")?,
            config_sha256: digest,
            // The retained receipt is audit history until this operation observes its provider.
            execution_id: field(resource, "provider_execution_id")
                .filter(|_| {
                    field(resource, "provider_known").and_then(Value::as_bool) == Some(true)
                })
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            execution_attempts: count("execution_attempts")?,
            resource: resource.clone(),
        })
    }

    /// Hash the exact requested bytes; whitespace changes require new proof too.
    pub fn change_digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.configuration.as_bytes()))
    }

    pub fn confirm_current(&self, current: &Value) -> Result<(), Error> {
        if required(current, "operation_key")? != self.operation_key
            || required(current, "status")? != required(&self.resource, "status")?
            || field(current, "operation_sequence").and_then(Value::as_u64) != Some(self.sequence)
            || required(current, "request_configuration")? != self.configuration
            || field(current, "request_revision")
                .and_then(Value::as_str)
                .unwrap_or("")
                != self.revision
            || required(current, "config_ref")? != self.config_ref
            || required(current, "config_sha256")? != self.config_sha256
            || required(current, "effort_id")? != self.effort_id
            || required(current, "proof_ref")? != self.proof_id
        {
            return Err(Error::Binding("resource no longer owns this invocation"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceConfig<T> {
    pub version: u64,
    pub resource_id: String,
    pub target: T,
    pub verification: Verification,
    #[serde(default)]
    pub required_ask_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub application: ApplicationBinding,
    pub flow: Flow,
    pub datadog: Datadog,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Flow {
    WebPage {
        path: String,
        required_text: String,
    },
    Story {
        story_id: String,
        world_id: String,
    },
    OperationalSnapshot {
        schema_version: String,
        secret_name: String,
    },
    Media {},
    ProviderConfiguration {},
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Datadog {
    pub site: String,
    pub service: String,
    pub environment: String,
    pub api_key_secret: String,
    pub app_key_secret: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationBinding {
    Unbound,
    Railway { resource_id: String, origin: String },
    Vercel { resource_id: String, origin: String },
}
