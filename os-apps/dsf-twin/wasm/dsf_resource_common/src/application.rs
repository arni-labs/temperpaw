//! Read-only proof that an application origin belongs to a registered resource.
use crate::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn application_origin(origin: &str) -> Result<url::Url, Error> {
    let url = url::Url::parse(origin).map_err(|_| Error::Binding("invalid application origin"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::Binding(
            "application requires a credential-free HTTPS origin",
        ));
    }
    Ok(url)
}

impl ApplicationBinding {
    pub fn validate_for<A: ResourceAction>(&self, invocation: &Invocation) -> Result<(), Error> {
        if A::ENTITY_TYPE == "DsfVercelProject" {
            self.vercel(&invocation.resource_id)?;
            return Ok(());
        }
        let own = A::ENTITY_TYPE == "DsfRailwayServiceInstance"
            && matches!(A::ACTION, "Deploy" | "Rollback");
        let (id, origin) = self.railway(own.then_some(invocation.resource_id.as_str()))?;
        if A::ENTITY_TYPE == "DsfMediaPipeline"
            && (id != required(&invocation.resource, "api_resource_id")?
                || origin.trim_end_matches('/') != DSF_API)
        {
            return Err(Error::Binding(
                "media verification must name its production API resource",
            ));
        }
        Ok(())
    }

    pub fn railway(&self, expected_id: Option<&str>) -> Result<(&str, &str), Error> {
        let Self::Railway {
            resource_id,
            origin,
        } = self
        else {
            return Err(Error::Binding(
                "verification requires a Railway application resource",
            ));
        };
        identifier(resource_id)?;
        application_origin(origin)?;
        if expected_id.is_some_and(|id| id != resource_id) {
            return Err(Error::Binding(
                "deployment verification names another application resource",
            ));
        }
        Ok((resource_id, origin))
    }
    pub fn vercel(&self, expected_id: &str) -> Result<&str, Error> {
        let Self::Vercel {
            resource_id,
            origin,
        } = self
        else {
            return Err(Error::Binding("verification requires its Vercel project"));
        };
        if resource_id != expected_id {
            return Err(Error::Binding("Vercel verification names another project"));
        }
        application_origin(origin)?;
        Ok(origin)
    }
}

/// A linked resource is resolved from its immutable registered config, not a caller-supplied URL.
pub fn railway_application_origin(
    runtime: &mut Runtime<impl Host>,
    binding: &ApplicationBinding,
    expected_id: Option<&str>,
) -> Result<String, Error> {
    let (id, origin) = binding.railway(expected_id)?;
    let url = application_origin(origin)?;
    let row = runtime.row("DsfRailwayServiceInstances", id)?;
    if required(&row, "status")? == "Retired" {
        return Err(Error::Binding("application resource is retired"));
    }
    let raw = runtime.read("Files", required(&row, "config_ref")?, true)?;
    if raw.status != 200 || raw.body.len() > 32768 {
        return Err(Error::Response("application configuration"));
    }
    if format!("{:x}", Sha256::digest(raw.body.as_bytes())) != required(&row, "config_sha256")? {
        return Err(Error::Binding("application configuration hash differs"));
    }
    let config: ResourceConfig<Value> = serde_json::from_str(&raw.body)
        .map_err(|_| Error::Binding("invalid application configuration"))?;
    if config.version != 3 || config.resource_id != id {
        return Err(Error::Binding("application configuration identity differs"));
    }
    let target = &config.target;
    for field in ["project_id", "service_id", "environment_id"] {
        identifier(required(target, field)?)?;
        if required(target, field)? != required(&row, field)? {
            return Err(Error::Binding("application provider identity differs"));
        }
    }
    let project = required(target, "project_id")?;
    let service = required(target, "service_id")?;
    let environment = required(target, "environment_id")?;
    let data=runtime.bearer_json(required(target,"token_secret")?,"POST","https://backboard.railway.com/graphql/v2".into(),json!({"query":"query DsfApplicationDomain($serviceId:String!,$environmentId:String!){service(id:$serviceId){id projectId} serviceInstance(serviceId:$serviceId,environmentId:$environmentId){serviceId environmentId domains{customDomains{id domain projectId serviceId environmentId deletedAt} serviceDomains{id domain projectId serviceId environmentId deletedAt}}}}","variables":{"serviceId":service,"environmentId":environment}}))?;
    if data
        .get("errors")
        .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
    {
        return Err(Error::Response("Railway application domains"));
    }
    let actual_service = data
        .pointer("/data/service")
        .ok_or(Error::Response("Railway application service"))?;
    let instance = data
        .pointer("/data/serviceInstance")
        .ok_or(Error::Response("Railway application instance"))?;
    if required(actual_service, "id")? != service
        || required(actual_service, "projectId")? != project
        || required(instance, "serviceId")? != service
        || required(instance, "environmentId")? != environment
    {
        return Err(Error::Binding("Railway application domain target differs"));
    }
    let host = url
        .host_str()
        .ok_or(Error::Binding("application domain missing"))?;
    let mut matched = false;
    for field in ["customDomains", "serviceDomains"] {
        let domains = instance
            .get("domains")
            .and_then(|d| d.get(field))
            .and_then(Value::as_array)
            .filter(|v| v.len() <= 100)
            .ok_or(Error::Response("Railway application domain collection"))?;
        matched |= domains.iter().any(|d| {
            d.get("domain").and_then(Value::as_str) == Some(host)
                && (d.get("projectId").and_then(Value::as_str) == Some(project)
                    || (field == "serviceDomains"
                        && d.get("projectId").is_some_and(Value::is_null)))
                && d.get("serviceId").and_then(Value::as_str) == Some(service)
                && d.get("environmentId").and_then(Value::as_str) == Some(environment)
                && d.get("deletedAt").is_some_and(Value::is_null)
        });
    }
    if !matched {
        return Err(Error::Binding(
            "application origin is not owned by the provider instance",
        ));
    }
    Ok(origin.trim_end_matches('/').into())
}
