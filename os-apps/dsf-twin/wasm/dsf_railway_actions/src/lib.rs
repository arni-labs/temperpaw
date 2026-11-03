//! Railway service-instance actions. Each stage WASM selects one concrete action.
use dsf_resource_common::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod configuration;
mod deployment;
pub use configuration::ApplyConfiguration;
pub use deployment::{Deploy, Rollback};

const API: &str = "https://backboard.railway.com/graphql/v2";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub project_id: String,
    pub service_id: String,
    pub environment_id: String,
    pub token_secret: String,
}

impl Target {
    fn validate(&self, resource: &Value) -> Result<(), Error> {
        for (field, expected) in [
            ("project_id", &self.project_id),
            ("service_id", &self.service_id),
            ("environment_id", &self.environment_id),
        ] {
            identifier(expected)?;
            if required(resource, field)? != expected {
                return Err(Error::Binding("Railway resource identity differs"));
            }
        }
        identifier(&self.token_secret)?;
        Ok(())
    }
    fn evidence(&self, deployment: Option<&str>) -> String {
        format!(
            "https://railway.com/project/{}/service/{}?environmentId={}{}",
            encoded(&self.project_id),
            encoded(&self.service_id),
            encoded(&self.environment_id),
            deployment
                .map(|id| format!("&id={}", encoded(id)))
                .unwrap_or_default()
        )
    }
    fn query(
        &self,
        runtime: &mut Runtime<impl Host>,
        query: &str,
        variables: Value,
    ) -> Result<Value, Error> {
        runtime.bearer_json(
            &self.token_secret,
            "POST",
            API.into(),
            json!({"query":query,"variables":variables}),
        )
    }
    fn instance(&self, runtime: &mut Runtime<impl Host>) -> Result<Value, Error> {
        let result = self.query(runtime,
            "query DsfRailwayInstance($serviceId:String!,$environmentId:String!){service(id:$serviceId){id projectId} serviceInstance(serviceId:$serviceId,environmentId:$environmentId){id serviceId environmentId startCommand buildCommand rootDirectory healthcheckPath healthcheckTimeout region numReplicas restartPolicyType restartPolicyMaxRetries sleepApplication latestDeployment{id} activeDeployments{id}}}",
            json!({"serviceId":self.service_id,"environmentId":self.environment_id}))?;
        let service = result
            .pointer("/data/service")
            .ok_or(Error::Response("Railway service"))?;
        if required(service, "projectId")? != self.project_id
            || required(service, "id")? != self.service_id
        {
            return Err(Error::Binding("Railway service belongs to another project"));
        }
        let instance = result
            .pointer("/data/serviceInstance")
            .ok_or(Error::Response("Railway service instance"))?;
        if required(instance, "serviceId")? != self.service_id
            || required(instance, "environmentId")? != self.environment_id
        {
            return Err(Error::Binding("Railway service instance differs"));
        }
        Ok(instance.clone())
    }
}

fn receipt(target: &Target, id: &str) -> Result<Receipt, Error> {
    identifier(id)?;
    Ok(Receipt {
        execution_id: id.into(),
        evidence_ref: target.evidence(Some(id)),
    })
}

#[cfg(test)]
mod tests;
