use super::*;

pub struct ApplyConfiguration;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    healthcheck_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    healthcheck_timeout: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    num_replicas: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_policy_type: Option<RestartPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart_policy_max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sleep_application: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum RestartPolicy {
    OnFailure,
    Always,
    Never,
}

impl Configuration {
    fn json(&self) -> Result<Value, Error> {
        serde_json::to_value(self)
            .map_err(|_| Error::Binding("Railway configuration serialization"))
    }
    fn matches(&self, actual: &Value) -> Result<bool, Error> {
        let desired = self.json()?;
        Ok(desired.as_object().is_some_and(|fields| {
            fields
                .iter()
                .all(|(name, value)| actual.get(name) == Some(value))
        }))
    }
}

impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = Configuration;
    const ENTITY_TYPE: &'static str = "DsfRailwayServiceInstance";
    const ENTITY_SET: &'static str = "DsfRailwayServiceInstances";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(target: &Target, resource: &Value) -> Result<(), Error> {
        target.validate(resource)
    }
    fn validate_change(_: &Target, change: &Configuration, _: &Invocation) -> Result<(), Error> {
        if change.json()?.as_object().is_none_or(|v| v.is_empty())
            || change.num_replicas == Some(0)
            || change.healthcheck_timeout == Some(0)
        {
            return Err(Error::Binding("empty or invalid Railway configuration"));
        }
        Ok(())
    }
    fn execute(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Configuration,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        let current = target.instance(runtime)?;
        if !change.matches(&current)? {
            let value = target.query(runtime, "mutation DsfRailwayConfiguration($serviceId:String!,$environmentId:String!,$input:ServiceInstanceUpdateInput!){serviceInstanceUpdate(serviceId:$serviceId,environmentId:$environmentId,input:$input)}",
                json!({"serviceId":target.service_id,"environmentId":target.environment_id,"input":change.json()?}))?;
            if value.pointer("/data/serviceInstanceUpdate") != Some(&json!(true)) {
                return Err(Error::Response("Railway configuration acceptance"));
            }
        }
        Ok(Receipt {
            execution_id: required(&current, "id")?.into(),
            evidence_ref: target.evidence(None),
        })
    }
    fn observe(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Configuration,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        let current = target.instance(runtime)?;
        if !change.matches(&current)? {
            return Err(Error::Absent(target.evidence(None)));
        }
        Ok(Receipt {
            execution_id: required(&current, "id")?.into(),
            evidence_ref: target.evidence(None),
        })
    }
    fn verify(
        runtime: &mut Runtime<impl Host>,
        target: &Target,
        change: &Configuration,
        invocation: &Invocation,
        verification: &Verification,
    ) -> Result<Evidence, Error> {
        let current = target.instance(runtime)?;
        if !change.matches(&current)? {
            return Err(Error::Pending("Railway configuration read differs"));
        }
        if invocation.execution_id.as_deref() != Some(required(&current, "id")?) {
            return Err(Error::Binding(
                "Railway service-instance execution identity differs",
            ));
        }
        verify_configuration(runtime, verification, invocation, target.evidence(None))
    }
}
