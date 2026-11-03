use crate::*;
use sha2::{Digest, Sha256};

impl<H: Host> Runtime<'_, H> {
    pub fn load_current<A: ResourceAction>(
        &mut self,
        invocation: &Invocation,
    ) -> Result<(ResourceConfig<A::Target>, A::Change), Error> {
        let current = self.row(A::ENTITY_SET, &invocation.resource_id)?;
        invocation.confirm_current(&current)?;
        let raw = self.read("Files", &invocation.config_ref, true)?;
        if raw.status != 200 || raw.body.len() > 32_768 {
            return Err(Error::Response("configuration File"));
        }
        if format!("{:x}", Sha256::digest(raw.body.as_bytes())) != invocation.config_sha256 {
            return Err(Error::Binding("configuration File hash changed"));
        }
        let config: ResourceConfig<A::Target> = serde_json::from_str(&raw.body)
            .map_err(|_| Error::Binding("invalid resource configuration"))?;
        if config.version != 3 || config.resource_id != invocation.resource_id {
            return Err(Error::Binding("configuration belongs to another resource"));
        }
        config
            .verification
            .application
            .validate_for::<A>(invocation)?;
        A::validate_target(&config.target, &current)?;
        let change: A::Change = serde_json::from_str(&invocation.configuration)
            .map_err(|_| Error::Binding("invalid typed change"))?;
        A::validate_change(&config.target, &change, invocation)?;
        Ok((config, change))
    }

    pub fn authorize<A: ResourceAction>(
        &mut self,
        invocation: &Invocation,
    ) -> Result<(ResourceConfig<A::Target>, A::Change), Error> {
        let (config, change) = self.load_current::<A>(invocation)?;
        let allowed = decoded(&invocation.resource, "allowed_operations")?;
        if !allowed
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(A::ACTION)))
        {
            return Err(Error::Binding("resource does not allow this action"));
        }
        let effort = self.row("Efforts", &invocation.effort_id)?;
        let proof = self.row("ProofPackets", &invocation.proof_id)?;
        validate_records(invocation, &effort, &proof)?;
        self.validate_asks(&invocation.effort_id, &config.required_ask_ids, &effort)?;
        A::validate_authority(self, &config, &change, invocation)?;
        let commit = required(&effort, "head_sha")?;
        chain_proof_ready::proof_packet_holds(&proof, Some(commit)).map_err(Error::Proof)?;
        let artifact = required(&proof, "artifact_ref")?;
        if required(&self.row("Files", artifact)?, "status")? != "Ready" {
            return Err(Error::Proof("artifact File is not Ready".into()));
        }
        let artifact = json_body(self.read("Files", artifact, true)?, "proof artifact")?;
        validate_change_proof::<A>(invocation, &artifact)?;
        Ok((config, change))
    }

    fn validate_asks(
        &mut self,
        effort_id: &str,
        required_ids: &[String],
        effort: &Value,
    ) -> Result<(), Error> {
        let raw = field(effort, "ask_ids")
            .map(|_| decoded(effort, "ask_ids"))
            .transpose()?
            .unwrap_or(json!([]));
        let ids = raw
            .as_array()
            .ok_or(Error::Binding("Effort Ask IDs are not an array"))?;
        if ids.len() > 32 || required_ids.len() > 32 {
            return Err(Error::Binding("Ask read budget exceeded"));
        }
        for needed in required_ids {
            if !ids.iter().any(|v| v.as_str() == Some(needed)) {
                return Err(Error::Binding("required Ask is not linked to Effort"));
            }
        }
        for id in ids {
            let id = id.as_str().ok_or(Error::Binding("invalid Ask ID"))?;
            let ask = self.row("Asks", id)?;
            if required(&ask, "effort_id")? != effort_id {
                return Err(Error::Binding("Ask belongs to another Effort"));
            }
            let status = required(&ask, "status")?;
            if status == "Open" && field(&ask, "stalls") == Some(&Value::Bool(true)) {
                return Err(Error::Blocked(id.into()));
            }
            if required_ids.iter().any(|r| r == id) {
                if status != "Answered" {
                    return Err(Error::Blocked(id.into()));
                }
                let choice = required(&ask, "chose")?;
                required(&ask, "who")?;
                if [
                    "no",
                    "deny",
                    "denied",
                    "reject",
                    "rejected",
                    "cancel",
                    "cancelled",
                ]
                .contains(&choice.trim().to_ascii_lowercase().as_str())
                {
                    return Err(Error::Blocked(id.into()));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_records(
    invocation: &Invocation,
    effort: &Value,
    proof: &Value,
) -> Result<(), Error> {
    if !["Merged", "Deploying", "Verified"].contains(&required(effort, "status")?)
        || !full_sha(required(effort, "head_sha")?)
    {
        return Err(Error::Binding(
            "Effort is not ready at a full source revision",
        ));
    }
    for flag in [
        "proof_attached",
        "e2e_ok",
        "review_passed",
        "evaluation_passed",
    ] {
        if field(effort, flag) != Some(&Value::Bool(true)) {
            return Err(Error::Binding(
                "Effort review or proof gate is not satisfied",
            ));
        }
    }
    let ids = field(effort, "proof_packet_ids")
        .map(|_| decoded(effort, "proof_packet_ids"))
        .transpose()?
        .unwrap_or(json!([]));
    if field(effort, "proof_packet_id").and_then(Value::as_str) != Some(&invocation.proof_id)
        && !ids
            .as_array()
            .is_some_and(|a| a.iter().any(|id| id.as_str() == Some(&invocation.proof_id)))
    {
        return Err(Error::Binding("proof is not attached to Effort"));
    }
    if required(proof, "effort_id")? != invocation.effort_id
        || required(proof, "commit")? != required(effort, "head_sha")?
    {
        return Err(Error::Binding(
            "proof belongs to another Effort or source revision",
        ));
    }
    Ok(())
}

pub(crate) fn validate_change_proof<A: ResourceAction>(
    invocation: &Invocation,
    artifact: &Value,
) -> Result<(), Error> {
    let binding = artifact.get("resource_change").ok_or(Error::Binding(
        "proof does not identify the resource change",
    ))?;
    for (name, expected) in [
        ("resource_id", invocation.resource_id.as_str()),
        ("entity_type", A::ENTITY_TYPE),
        ("action", A::ACTION),
        ("operation_key", invocation.operation_key.as_str()),
        ("revision", invocation.revision.as_str()),
        ("configuration_sha256", invocation.change_digest().as_str()),
    ] {
        if binding.get(name).and_then(Value::as_str) != Some(expected) {
            return Err(Error::Binding(
                "proof identifies a different resource change",
            ));
        }
    }
    if binding.get("operation_sequence").and_then(Value::as_u64) != Some(invocation.sequence) {
        return Err(Error::Binding(
            "proof identifies another operation sequence",
        ));
    }
    Ok(())
}
