//! Proof, identity and transport boundaries for resource-owned actions.
//! Provider selection happens in each WASM's concrete Rust type.
mod application;
mod authority;
mod invocation;
mod transport;
mod verification;

pub use application::*;
pub use invocation::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
pub use transport::*;
pub use verification::*;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid resource binding: {0}")]
    Binding(&'static str),
    #[error("missing field: {0}")]
    Field(String),
    #[error("provider transport unavailable")]
    Transport,
    #[error("HTTP {0} from {1}")]
    Http(u16, &'static str),
    #[error("invalid {0} response")]
    Response(&'static str),
    #[error("proof does not pass: {0}")]
    Proof(String),
    #[error("required Ask {0} is unresolved or declines the operation")]
    Blocked(String),
    #[error("provider operation has failed: {0}")]
    ProviderFailed(&'static str),
    #[error("verification pending: {0}")]
    Pending(&'static str),
    #[error("provider confirms operation absent")]
    Absent(String),
}

/// One statically selected action on one provider resource type.
pub trait ResourceAction {
    type Target: DeserializeOwned;
    type Change: DeserializeOwned;
    const ENTITY_TYPE: &'static str;
    const ENTITY_SET: &'static str;
    const ACTION: &'static str;
    const RESULT: VerifiedValue;

    /// Reject changes whose typed target differs from the registered resource.
    fn validate_target(target: &Self::Target, resource: &Value) -> Result<(), Error>;
    fn validate_change(
        target: &Self::Target,
        change: &Self::Change,
        invocation: &Invocation,
    ) -> Result<(), Error>;
    /// Action-specific authority checks beyond the shared Effort and proof gates.
    fn validate_authority(
        _runtime: &mut Runtime<impl Host>,
        _config: &ResourceConfig<Self::Target>,
        _change: &Self::Change,
        _invocation: &Invocation,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn execute(
        runtime: &mut Runtime<impl Host>,
        target: &Self::Target,
        change: &Self::Change,
        invocation: &Invocation,
    ) -> Result<Receipt, Error>;
    fn observe(
        runtime: &mut Runtime<impl Host>,
        target: &Self::Target,
        change: &Self::Change,
        invocation: &Invocation,
    ) -> Result<Receipt, Error>;
    fn verify(
        runtime: &mut Runtime<impl Host>,
        target: &Self::Target,
        change: &Self::Change,
        invocation: &Invocation,
        verification: &Verification,
    ) -> Result<Evidence, Error>;
}

pub struct Receipt {
    pub execution_id: String,
    pub evidence_ref: String,
}

pub enum VerifiedValue {
    Revision,
    Configuration,
}

pub struct Evidence {
    pub provider_ref: String,
    pub flow_ref: String,
    pub telemetry_ref: String,
    pub observed_revision: String,
    pub observed_configuration: String,
}

pub struct Callback {
    pub action: String,
    pub params: Value,
}

impl Invocation {
    /// Every result carries the sequence captured before the provider request.
    pub fn callback<A: ResourceAction>(&self, suffix: &str, mut params: Value) -> Callback {
        let fields = params.as_object_mut().expect("internal callback object");
        fields.insert("operation_key".into(), self.operation_key.clone().into());
        fields.insert("expected_operation_sequence".into(), self.sequence.into());
        Callback {
            action: format!("{}{suffix}", A::ACTION),
            params,
        }
    }
}

pub fn validate<A: ResourceAction>(
    runtime: &mut Runtime<impl Host>,
    invocation: &Invocation,
) -> Result<Callback, Error> {
    invocation.require_stage::<A>("Validating")?;
    runtime.authorize::<A>(invocation)?;
    let mut params = json!({
        "validation_evidence_ref": format!("{}/tdata/ProofPackets('{}')", runtime.base, invocation.proof_id)
    });
    match A::RESULT {
        VerifiedValue::Revision => params["intended_revision"] = invocation.revision.clone().into(),
        VerifiedValue::Configuration => {
            params["intended_configuration"] = invocation.configuration.clone().into()
        }
    }
    Ok(invocation.callback::<A>("ValidationSucceeded", params))
}

pub fn execute<A: ResourceAction>(
    runtime: &mut Runtime<impl Host>,
    invocation: &Invocation,
) -> Result<Callback, Error> {
    invocation.require_stage::<A>("Executing")?;
    let (config, change) = runtime.authorize::<A>(invocation)?;
    let receipt = A::execute(runtime, &config.target, &change, invocation)?;
    Ok(invocation.callback::<A>(
        "ExecutionSucceeded",
        json!({
            "provider_execution_id": receipt.execution_id,
            "provider_evidence_ref": receipt.evidence_ref
        }),
    ))
}

pub fn observe<A: ResourceAction>(
    runtime: &mut Runtime<impl Host>,
    invocation: &Invocation,
) -> Result<Callback, Error> {
    invocation.require_stage::<A>("Reconciling")?;
    let (config, change) = runtime.load_current::<A>(invocation)?;
    let receipt = match A::observe(runtime, &config.target, &change, invocation) {
        Ok(receipt) => receipt,
        Err(Error::Absent(evidence_ref)) => {
            return Ok(invocation.callback::<A>(
                "ProviderAbsent",
                json!({"absence_evidence_ref":evidence_ref}),
            ));
        }
        Err(error) => return Err(error),
    };
    Ok(invocation.callback::<A>(
        "ProviderFound",
        json!({
            "provider_execution_id": receipt.execution_id,
            "provider_evidence_ref": receipt.evidence_ref
        }),
    ))
}

pub fn verify<A: ResourceAction>(
    runtime: &mut Runtime<impl Host>,
    invocation: &Invocation,
) -> Result<Callback, Error> {
    invocation.require_stage::<A>("Verifying")?;
    let (config, change) = runtime.load_current::<A>(invocation)?;
    let evidence = A::verify(
        runtime,
        &config.target,
        &change,
        invocation,
        &config.verification,
    )?;
    let mut params = json!({
        "verified_resource_id": invocation.resource_id,
        "provider_evidence_ref": evidence.provider_ref,
        "flow_evidence_ref": evidence.flow_ref,
        "telemetry_evidence_ref": evidence.telemetry_ref
    });
    match A::RESULT {
        VerifiedValue::Revision => params["verified_revision"] = evidence.observed_revision.into(),
        VerifiedValue::Configuration => {
            params["verified_configuration"] = evidence.observed_configuration.into()
        }
    }
    Ok(invocation.callback::<A>("VerificationSucceeded", params))
}

#[cfg(target_arch = "wasm32")]
pub mod guest;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod application_tests;
