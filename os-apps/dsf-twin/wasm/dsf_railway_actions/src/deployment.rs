use super::*;
use chrono::DateTime;

pub struct Deploy;
pub struct Rollback;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployChange {
    baseline_deployment_id: String,
    not_before_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackChange {
    baseline_deployment_id: String,
    deployment_id: String,
}

struct Deployment {
    id: String,
    status: String,
    revision: String,
    snapshot_id: Option<String>,
    can_rollback: bool,
}
impl Deployment {
    fn parse(target: &Target, value: &Value) -> Result<Self, Error> {
        for (field, expected) in [
            ("projectId", &target.project_id),
            ("serviceId", &target.service_id),
            ("environmentId", &target.environment_id),
        ] {
            if required(value, field)? != expected {
                return Err(Error::Binding(
                    "Railway deployment belongs to another target",
                ));
            }
        }
        Ok(Self {
            id: required(value, "id")?.into(),
            status: required(value, "status")?.into(),
            revision: value
                .pointer("/meta/commitHash")
                .and_then(Value::as_str)
                .ok_or(Error::Response("Railway commit"))?
                .into(),
            snapshot_id: value
                .get("snapshotId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            can_rollback: value.get("canRollback") == Some(&json!(true)),
        })
    }
    fn ready(&self, revision: &str) -> Result<(), Error> {
        if ["FAILED", "CRASHED", "REMOVED"].contains(&self.status.as_str()) {
            return Err(Error::ProviderFailed(
                "Railway reports terminal deployment failure",
            ));
        }
        if self.status != "SUCCESS" {
            return Err(Error::Pending("Railway deployment is not successful"));
        }
        if self.revision != revision {
            return Err(Error::Binding("Railway deployed revision differs"));
        }
        Ok(())
    }
}

fn get(runtime: &mut Runtime<impl Host>, target: &Target, id: &str) -> Result<Deployment, Error> {
    identifier(id)?;
    let value = target.query(runtime, "query DsfRailwayDeployment($id:String!){deployment(id:$id){id status projectId serviceId environmentId snapshotId meta canRollback}}", json!({"id":id}))?;
    Deployment::parse(
        target,
        value
            .pointer("/data/deployment")
            .ok_or(Error::Response("Railway deployment"))?,
    )
}

fn find_deploy(
    runtime: &mut Runtime<impl Host>,
    target: &Target,
    change: &DeployChange,
    invocation: &Invocation,
) -> Result<Option<Deployment>, Error> {
    if let Some(id) = &invocation.execution_id {
        let found = get(runtime, target, id)?;
        if found.revision != invocation.revision {
            return Err(Error::Binding("Railway deployment revision differs"));
        }
        return Ok(Some(found));
    }
    let value = target.query(runtime, "query DsfRailwayDeployments($input:DeploymentListInput!){deployments(input:$input,first:50){edges{node{id status projectId serviceId environmentId snapshotId meta createdAt canRollback}}}}",
        json!({"input":{"projectId":target.project_id,"serviceId":target.service_id,"environmentId":target.environment_id}}))?;
    let entries = value
        .pointer("/data/deployments/edges")
        .and_then(Value::as_array)
        .ok_or(Error::Response("Railway deployments"))?;
    let mut found = None;
    for edge in entries.iter().take(50) {
        let raw = edge
            .get("node")
            .ok_or(Error::Response("Railway deployment edge"))?;
        if raw.pointer("/meta/commitHash").and_then(Value::as_str) != Some(&invocation.revision) {
            continue;
        }
        let candidate = Deployment::parse(target, raw)?;
        if candidate.id == change.baseline_deployment_id
            || candidate.revision != invocation.revision
        {
            continue;
        }
        let created = DateTime::parse_from_rfc3339(required(raw, "createdAt")?)
            .map_err(|_| Error::Response("Railway deployment creation time"))?
            .timestamp_millis();
        if created < change.not_before_ms {
            continue;
        }
        if found.is_some() {
            return Err(Error::Pending(
                "multiple Railway deployments match the operation",
            ));
        }
        found = Some(candidate);
    }
    Ok(found)
}

impl ResourceAction for Deploy {
    type Target = Target;
    type Change = DeployChange;
    const ENTITY_TYPE: &'static str = "DsfRailwayServiceInstance";
    const ENTITY_SET: &'static str = "DsfRailwayServiceInstances";
    const ACTION: &'static str = "Deploy";
    const RESULT: VerifiedValue = VerifiedValue::Revision;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.validate(resource)
    }
    fn validate_change(
        _: &Target,
        change: &DeployChange,
        invocation: &Invocation,
    ) -> Result<(), Error> {
        identifier(&change.baseline_deployment_id)?;
        if !full_sha(&invocation.revision) || change.not_before_ms <= 0 {
            return Err(Error::Binding(
                "Railway deployment requires full revision and positive time bound",
            ));
        }
        Ok(())
    }
    fn execute(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &DeployChange,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        if change.not_before_ms > runtime.now_ms + 5000 {
            return Err(Error::Binding("Railway deployment time is in the future"));
        }
        if let Some(found) = find_deploy(runtime, target, change, invocation)? {
            return receipt(target, &found.id);
        }
        if invocation.execution_attempts != 1 {
            return Err(Error::Pending(
                "Railway deployment write is not repeated after ambiguity",
            ));
        }
        let instance = target.instance(runtime)?;
        if instance
            .pointer("/latestDeployment/id")
            .and_then(Value::as_str)
            != Some(&change.baseline_deployment_id)
        {
            return Err(Error::Binding("Railway baseline changed before deployment"));
        }
        let value=target.query(runtime,"mutation DsfRailwayDeploy($serviceId:String!,$environmentId:String!,$commitSha:String!){serviceInstanceDeployV2(serviceId:$serviceId,environmentId:$environmentId,commitSha:$commitSha)}",
            json!({"serviceId":target.service_id,"environmentId":target.environment_id,"commitSha":invocation.revision}))?;
        let id = value
            .pointer("/data/serviceInstanceDeployV2")
            .and_then(Value::as_str)
            .ok_or(Error::Response("Railway deployment ID"))?;
        receipt(target, id)
    }
    fn observe(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &DeployChange,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        let found = find_deploy(runtime, target, change, invocation)?
            .ok_or(Error::Pending("no unique correlated Railway deployment"))?;
        receipt(target, &found.id)
    }
    fn verify(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &DeployChange,
        invocation: &Invocation,
        verification: &Verification,
    ) -> Result<Evidence, Error> {
        let found = find_deploy(runtime, target, change, invocation)?
            .ok_or(Error::Pending("Railway deployment not visible"))?;
        found.ready(&invocation.revision)?;
        if invocation.execution_id.as_deref() != Some(&found.id) {
            return Err(Error::Binding("Railway execution identity differs"));
        }
        let instance = target.instance(runtime)?;
        if !active_ids(&instance)?.iter().any(|id| *id == found.id) {
            return Err(Error::Pending("Railway deployment is not active"));
        }
        let (_, origin) = verification
            .application
            .railway(Some(&invocation.resource_id))?;
        let (flow_ref, telemetry_ref, revision) = verify_product(
            runtime,
            verification,
            invocation,
            origin,
            Some(&invocation.revision),
        )?;
        Ok(Evidence {
            provider_ref: target.evidence(Some(&found.id)),
            flow_ref,
            telemetry_ref,
            observed_revision: revision,
            observed_configuration: String::new(),
        })
    }
}

fn active_ids(instance: &Value) -> Result<Vec<&str>, Error> {
    let ids = instance
        .get("activeDeployments")
        .and_then(Value::as_array)
        .ok_or(Error::Response("Railway active deployments"))?;
    if ids.len() > 20 {
        return Err(Error::Binding(
            "Railway active deployment read budget exceeded",
        ));
    }
    ids.iter().map(|row| required(row, "id")).collect()
}

fn find_rollback(
    runtime: &mut Runtime<impl Host>,
    target: &Target,
    change: &RollbackChange,
    invocation: &Invocation,
) -> Result<Option<Deployment>, Error> {
    let original = get(runtime, target, &change.deployment_id)?;
    if original.revision != invocation.revision {
        return Err(Error::Binding("rollback source revision differs"));
    }
    let instance = target.instance(runtime)?;
    let mut found = None;
    for id in active_ids(&instance)? {
        if id == change.baseline_deployment_id && id != change.deployment_id {
            continue;
        }
        let active = get(runtime, target, id)?;
        if active.revision != original.revision
            || (active.id != original.id
                && (original.snapshot_id.is_none() || active.snapshot_id != original.snapshot_id))
        {
            continue;
        }
        if found.is_some() {
            return Err(Error::Pending(
                "multiple active Railway rollback candidates",
            ));
        }
        found = Some(active);
    }
    Ok(found)
}

impl ResourceAction for Rollback {
    type Target = Target;
    type Change = RollbackChange;
    const ENTITY_TYPE: &'static str = "DsfRailwayServiceInstance";
    const ENTITY_SET: &'static str = "DsfRailwayServiceInstances";
    const ACTION: &'static str = "Rollback";
    const RESULT: VerifiedValue = VerifiedValue::Revision;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.validate(resource)
    }
    fn validate_change(
        _: &Target,
        change: &RollbackChange,
        invocation: &Invocation,
    ) -> Result<(), Error> {
        identifier(&change.deployment_id)?;
        identifier(&change.baseline_deployment_id)?;
        if !full_sha(&invocation.revision)
            || required(&invocation.resource, "rollback_execution_id")? != change.deployment_id
        {
            return Err(Error::Binding(
                "rollback target differs from resource request",
            ));
        }
        Ok(())
    }
    fn execute(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &RollbackChange,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        if let Some(found) = find_rollback(runtime, target, change, invocation)? {
            return receipt(target, &found.id);
        }
        if invocation.execution_attempts != 1 {
            return Err(Error::Pending(
                "Railway rollback write is not repeated after ambiguity",
            ));
        }
        let candidate = get(runtime, target, &change.deployment_id)?;
        if !candidate.can_rollback {
            return Err(Error::Binding(
                "Railway target is not eligible for rollback",
            ));
        }
        let instance = target.instance(runtime)?;
        if instance
            .pointer("/latestDeployment/id")
            .and_then(Value::as_str)
            != Some(&change.baseline_deployment_id)
        {
            return Err(Error::Binding("Railway baseline changed before rollback"));
        }
        let value = target.query(
            runtime,
            "mutation DsfRailwayRollback($id:String!){deploymentRollback(id:$id)}",
            json!({"id":change.deployment_id}),
        )?;
        if value.pointer("/data/deploymentRollback") != Some(&json!(true)) {
            return Err(Error::Response("Railway rollback acceptance"));
        }
        let found = find_rollback(runtime, target, change, invocation)?.ok_or(Error::Pending(
            "Railway accepted rollback; active deployment not yet established",
        ))?;
        receipt(target, &found.id)
    }
    fn observe(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &RollbackChange,
        invocation: &Invocation,
    ) -> Result<Receipt, Error> {
        let found = find_rollback(runtime, target, change, invocation)?
            .ok_or(Error::Pending("Railway rollback is not active"))?;
        receipt(target, &found.id)
    }
    fn verify(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &RollbackChange,
        invocation: &Invocation,
        verification: &Verification,
    ) -> Result<Evidence, Error> {
        let found = find_rollback(runtime, target, change, invocation)?
            .ok_or(Error::Pending("Railway rollback is not active"))?;
        found.ready(&invocation.revision)?;
        if invocation.execution_id.as_deref() != Some(&found.id) {
            return Err(Error::Binding(
                "Railway rollback execution identity differs",
            ));
        }
        let (_, origin) = verification
            .application
            .railway(Some(&invocation.resource_id))?;
        let (flow_ref, telemetry_ref, revision) = verify_product(
            runtime,
            verification,
            invocation,
            origin,
            Some(&invocation.revision),
        )?;
        Ok(Evidence {
            provider_ref: target.evidence(Some(&found.id)),
            flow_ref,
            telemetry_ref,
            observed_revision: revision,
            observed_configuration: String::new(),
        })
    }
}
