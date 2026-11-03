//! Vercel project actions. Every request uses the registered project and team.
//! API shapes: vercel/sdk src/funcs/{deploymentsCreateDeployment,
//! projectsUpdateProject,projectsRequestRollback,aliasesAssignAlias}.ts.
use dsf_resource_common::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub project_id: String,
    pub account_id: String,
    pub project_name: String,
    pub git_repository_id: u64,
    pub token_secret: String,
    #[serde(default)]
    pub allowed_aliases: Vec<String>,
}
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentTarget {
    Production,
    Preview,
}
impl DeploymentTarget {
    fn name(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Preview => "preview",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeployChange {
    pub target: DeploymentTarget,
    pub baseline_deployment_id: String,
    pub not_before_ms: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationChange {
    pub target: DeploymentTarget,
    pub build_command: Option<String>,
    pub install_command: Option<String>,
    pub output_directory: Option<String>,
    pub root_directory: Option<String>,
    pub framework: Option<String>,
    pub node_version: Option<String>,
}
impl ConfigurationChange {
    fn body(&self) -> Value {
        let mut body = json!({});
        for (key, value) in [
            ("buildCommand", &self.build_command),
            ("installCommand", &self.install_command),
            ("outputDirectory", &self.output_directory),
            ("rootDirectory", &self.root_directory),
            ("framework", &self.framework),
            ("nodeVersion", &self.node_version),
        ] {
            if let Some(value) = value {
                body[key] = json!(value);
            }
        }
        body
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackChange {
    pub target: DeploymentTarget,
    pub deployment_id: String,
    pub baseline_deployment_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AliasChange {
    pub target: DeploymentTarget,
    pub alias: String,
    pub deployment_id: String,
    pub revision: String,
}
pub struct Deploy;
pub struct ApplyConfiguration;
pub struct Rollback;
pub struct SetAlias;

fn validate_target(target: &Target, row: &Value) -> Result<(), Error> {
    for id in [
        &target.project_id,
        &target.account_id,
        &target.project_name,
        &target.token_secret,
    ] {
        identifier(id)?;
    }
    if required(row, "project_id")? != target.project_id
        || required(row, "account_id")? != target.account_id
        || target.git_repository_id == 0
    {
        return Err(Error::Binding(
            "Vercel project identity differs from resource",
        ));
    }
    if target.allowed_aliases.len() > 20
        || target.allowed_aliases.iter().any(|name| !hostname(name))
    {
        return Err(Error::Binding("invalid Vercel alias allowlist"));
    }
    Ok(())
}
fn hostname(name: &str) -> bool {
    name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
fn validate_scope(target: DeploymentTarget, invocation: &Invocation) -> Result<(), Error> {
    if required(&invocation.resource, "deployment_target")? != target.name() {
        return Err(Error::Binding(
            "deployment target differs from accepted action",
        ));
    }
    Ok(())
}
fn project_url(target: &Target) -> String {
    format!(
        "https://api.vercel.com/v9/projects/{}?teamId={}",
        encoded(&target.project_id),
        encoded(&target.account_id)
    )
}
fn deployment_url(target: &Target, id: &str) -> String {
    format!(
        "https://api.vercel.com/v13/deployments/{}?teamId={}",
        encoded(id),
        encoded(&target.account_id)
    )
}
fn project(runtime: &mut Runtime<impl Host>, target: &Target) -> Result<Value, Error> {
    let row = runtime.bearer_json(
        &target.token_secret,
        "GET",
        project_url(target),
        Value::Null,
    )?;
    check_project(target, &row)?;
    Ok(row)
}
fn check_project(target: &Target, row: &Value) -> Result<(), Error> {
    if required(row, "id")? != target.project_id
        || required(row, "accountId")? != target.account_id
        || required(row, "name")? != target.project_name
    {
        return Err(Error::Binding(
            "Vercel returned a different project or account",
        ));
    }
    Ok(())
}
fn production_id(row: &Value) -> Result<&str, Error> {
    row.pointer("/targets/production/id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(Error::Response("Vercel production target"))
}
fn check_repository(target: &Target, row: &Value) -> Result<(), Error> {
    let repo = row.pointer("/link/repoId");
    let repo_id = repo
        .and_then(Value::as_u64)
        .or_else(|| repo.and_then(Value::as_str).and_then(|s| s.parse().ok()));
    if row.pointer("/link/type").and_then(Value::as_str) != Some("github")
        || repo_id != Some(target.git_repository_id)
    {
        return Err(Error::Binding("Vercel project GitHub repository differs"));
    }
    Ok(())
}
fn deployment(
    runtime: &mut Runtime<impl Host>,
    target: &Target,
    id: &str,
    scope: DeploymentTarget,
    revision: &str,
) -> Result<Value, Error> {
    identifier(id)?;
    let row = runtime.bearer_json(
        &target.token_secret,
        "GET",
        deployment_url(target, id),
        Value::Null,
    )?;
    check_deployment(target, &row, scope, revision)?;
    if required(&row, "id")? != id {
        return Err(Error::Binding("Vercel returned another deployment"));
    }
    Ok(row)
}
fn check_deployment(
    target: &Target,
    row: &Value,
    scope: DeploymentTarget,
    revision: &str,
) -> Result<(), Error> {
    if required(row, "projectId")? != target.project_id {
        return Err(Error::Binding(
            "deployment belongs to another Vercel project",
        ));
    }
    // The provider represents ordinary previews with a missing or null target.
    let actual = row.get("target");
    let correct = match scope {
        DeploymentTarget::Production => actual.and_then(Value::as_str) == Some("production"),
        DeploymentTarget::Preview => {
            actual.is_none_or(Value::is_null) || actual.and_then(Value::as_str) == Some("preview")
        }
    };
    if !correct || row.get("customEnvironment").is_some_and(|v| !v.is_null()) {
        return Err(Error::Binding("deployment environment differs"));
    }
    let actual_sha = row
        .pointer("/gitSource/sha")
        .and_then(Value::as_str)
        .or_else(|| row.pointer("/meta/githubCommitSha").and_then(Value::as_str));
    if !full_sha(revision) || actual_sha != Some(revision) {
        return Err(Error::Binding("Vercel deployment revision differs"));
    }
    if let Some(other) = row.pointer("/meta/githubCommitSha").and_then(Value::as_str)
        && other != revision
    {
        return Err(Error::Binding("conflicting Vercel deployment revisions"));
    }
    identifier(required(row, "id")?)?;
    Ok(())
}
fn ready(row: &Value) -> Result<(), Error> {
    match required(row, "readyState")? {
        "READY" => Ok(()),
        "ERROR" | "CANCELED" => Err(Error::ProviderFailed("Vercel deployment")),
        "QUEUED" | "INITIALIZING" | "BUILDING" => {
            Err(Error::Pending("Vercel deployment is not ready"))
        }
        _ => Err(Error::Response("Vercel deployment readyState")),
    }
}
fn receipt(target: &Target, id: &str) -> Receipt {
    Receipt {
        execution_id: id.into(),
        evidence_ref: deployment_url(target, id),
    }
}
fn correlated(row: &Value, invocation: &Invocation) -> bool {
    row.pointer("/meta/dsfOperationKey").and_then(Value::as_str) == Some(&invocation.operation_key)
        && row
            .pointer("/meta/dsfOperationSequence")
            .and_then(Value::as_str)
            == Some(invocation.sequence.to_string().as_str())
}
fn check_created(row: &Value, invocation: &Invocation, change: &DeployChange) -> Result<(), Error> {
    if !correlated(row, invocation)
        || row
            .get("createdAt")
            .and_then(Value::as_i64)
            .is_none_or(|ts| ts < change.not_before_ms)
    {
        return Err(Error::Binding("deployment does not match this operation"));
    }
    Ok(())
}
fn find_deployment(
    runtime: &mut Runtime<impl Host>,
    target: &Target,
    change: &DeployChange,
    invocation: &Invocation,
) -> Result<Option<Value>, Error> {
    if let Some(id) = &invocation.execution_id {
        let row = deployment(runtime, target, id, change.target, &invocation.revision)?;
        check_created(&row, invocation, change)?;
        return Ok(Some(row));
    }
    let url = format!(
        "https://api.vercel.com/v6/deployments?projectId={}&teamId={}&limit=20&meta-dsfOperationKey={}",
        encoded(&target.project_id),
        encoded(&target.account_id),
        encoded(&invocation.operation_key)
    );
    let list = runtime.bearer_json(&target.token_secret, "GET", url, Value::Null)?;
    let rows = list
        .get("deployments")
        .and_then(Value::as_array)
        .ok_or(Error::Response("Vercel deployment list"))?;
    if rows.len() >= 20
        || list
            .pointer("/pagination/next")
            .is_some_and(|v| !v.is_null())
    {
        return Err(Error::Pending("Vercel deployment search is incomplete"));
    }
    let matches: Vec<_> = rows.iter().filter(|v| correlated(v, invocation)).collect();
    if matches.len() > 1 {
        return Err(Error::Pending(
            "multiple Vercel deployments share operation identity",
        ));
    }
    let Some(found) = matches.first() else {
        return Ok(None);
    };
    let id = found
        .get("uid")
        .or_else(|| found.get("id"))
        .and_then(Value::as_str)
        .ok_or(Error::Response("Vercel deployment list identity"))?;
    let row = deployment(runtime, target, id, change.target, &invocation.revision)?;
    check_created(&row, invocation, change)?;
    Ok(Some(row))
}
fn verify_deployment(
    runtime: &mut Runtime<impl Host>,
    target: &Target,
    scope: DeploymentTarget,
    invocation: &Invocation,
    verification: &Verification,
    row: &Value,
    revision: &str,
) -> Result<Evidence, Error> {
    ready(row)?;
    let (flow_ref, telemetry_ref, observed_revision) = match scope {
        DeploymentTarget::Production => {
            let origin = verification.application.vercel(&invocation.resource_id)?;
            let parsed = application_origin(origin)?;
            let alias = parsed
                .host_str()
                .ok_or(Error::Binding("Vercel application alias missing"))?;
            let exact = AliasChange {
                alias: alias.into(),
                deployment_id: required(row, "id")?.into(),
                revision: revision.into(),
                target: scope,
            };
            if !target
                .allowed_aliases
                .iter()
                .any(|allowed| allowed == alias)
                || !alias_matches(runtime, target, &exact)?
            {
                return Err(Error::Binding(
                    "Vercel application alias does not route to this deployment",
                ));
            }
            verify_vercel_alias(
                runtime,
                verification,
                invocation,
                origin,
                &target.allowed_aliases,
                revision,
            )?
        }
        DeploymentTarget::Preview => {
            let domain = required(row, "url")?;
            if !hostname(domain) || !domain.ends_with(".vercel.app") {
                return Err(Error::Binding("invalid provider preview URL"));
            }
            verify_vercel_preview(
                runtime,
                verification,
                invocation,
                &format!("https://{domain}"),
                revision,
            )?
        }
    };
    Ok(Evidence {
        provider_ref: deployment_url(target, required(row, "id")?),
        flow_ref,
        telemetry_ref,
        observed_revision,
        observed_configuration: invocation.configuration.clone(),
    })
}

impl ResourceAction for Deploy {
    type Target = Target;
    type Change = DeployChange;
    const ENTITY_TYPE: &'static str = "DsfVercelProject";
    const ENTITY_SET: &'static str = "DsfVercelProjects";
    const ACTION: &'static str = "Deploy";
    const RESULT: VerifiedValue = VerifiedValue::Revision;
    fn validate_target(t: &Target, r: &Value) -> Result<(), Error> {
        validate_target(t, r)
    }
    fn validate_change(_: &Target, c: &DeployChange, i: &Invocation) -> Result<(), Error> {
        validate_scope(c.target, i)?;
        identifier(&c.baseline_deployment_id)?;
        if !full_sha(&i.revision) || c.not_before_ms <= 0 {
            return Err(Error::Binding(
                "deployment requires exact revision and start time",
            ));
        }
        Ok(())
    }
    fn execute(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &DeployChange,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        let p = project(r, t)?;
        check_repository(t, &p)?;
        if c.not_before_ms > r.now_ms {
            return Err(Error::Binding("deployment start time is in the future"));
        }
        if let Some(row) = find_deployment(r, t, c, i)? {
            return Ok(receipt(t, required(&row, "id")?));
        }
        if i.execution_attempts != 1 {
            return Err(Error::Pending(
                "unconfirmed deployment creation requires reconciliation",
            ));
        }
        if c.target == DeploymentTarget::Production
            && production_id(&p)? != c.baseline_deployment_id
        {
            return Err(Error::Binding(
                "production changed since deployment was planned",
            ));
        }
        let mut body = json!({"name":t.project_name,"project":t.project_id,"gitSource":{"type":"github","repoId":t.git_repository_id,"sha":i.revision},"meta":{"dsfOperationKey":i.operation_key,"dsfOperationSequence":i.sequence.to_string(),"dsfEffortId":i.effort_id}});
        if c.target == DeploymentTarget::Production {
            body["target"] = json!("production");
        }
        let row = r.bearer_json(
            &t.token_secret,
            "POST",
            format!(
                "https://api.vercel.com/v13/deployments?teamId={}",
                encoded(&t.account_id)
            ),
            body,
        )?;
        check_deployment(t, &row, c.target, &i.revision)?;
        check_created(&row, i, c)?;
        Ok(receipt(t, required(&row, "id")?))
    }
    fn observe(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &DeployChange,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        project(r, t)?;
        let row = find_deployment(r, t, c, i)?.ok_or(Error::Pending(
            "provider has not exposed a correlated deployment",
        ))?;
        Ok(receipt(t, required(&row, "id")?))
    }
    fn verify(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &DeployChange,
        i: &Invocation,
        v: &Verification,
    ) -> Result<Evidence, Error> {
        let p = project(r, t)?;
        let row = find_deployment(r, t, c, i)?.ok_or(Error::Pending("deployment not found"))?;
        if c.target == DeploymentTarget::Production && production_id(&p)? != required(&row, "id")? {
            return Err(Error::Pending("production has not switched to deployment"));
        }
        verify_deployment(r, t, c.target, i, v, &row, &i.revision)
    }
}

impl ResourceAction for ApplyConfiguration {
    type Target = Target;
    type Change = ConfigurationChange;
    const ENTITY_TYPE: &'static str = "DsfVercelProject";
    const ENTITY_SET: &'static str = "DsfVercelProjects";
    const ACTION: &'static str = "ApplyConfiguration";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(t: &Target, r: &Value) -> Result<(), Error> {
        validate_target(t, r)
    }
    fn validate_change(_: &Target, c: &ConfigurationChange, i: &Invocation) -> Result<(), Error> {
        validate_scope(c.target, i)?;
        let body = c.body();
        let fields = body.as_object().expect("constructed object");
        if fields.is_empty()
            || fields.values().any(|v| {
                v.as_str()
                    .is_none_or(|s| s.len() > 4096 || s.contains('\0'))
            })
        {
            return Err(Error::Binding(
                "empty or oversized Vercel project configuration",
            ));
        }
        if c.node_version.as_ref().is_some_and(|s| {
            s.len() > 10
                || !s.ends_with(".x")
                || !s[..s.len() - 2].bytes().all(|b| b.is_ascii_digit())
        }) {
            return Err(Error::Binding("invalid Node version"));
        }
        Ok(())
    }
    fn execute(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &ConfigurationChange,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        project(r, t)?;
        let row = r.bearer_json(&t.token_secret, "PATCH", project_url(t), c.body())?;
        check_project(t, &row)?;
        Ok(Receipt {
            execution_id: project_url(t),
            evidence_ref: project_url(t),
        })
    }
    fn observe(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &ConfigurationChange,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        let row = project(r, t)?;
        compare_configuration(&row, c).map_err(|e| {
            if matches!(e, Error::Pending(_)) {
                Error::Absent(project_url(t))
            } else {
                e
            }
        })?;
        Ok(Receipt {
            execution_id: project_url(t),
            evidence_ref: project_url(t),
        })
    }
    fn verify(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &ConfigurationChange,
        i: &Invocation,
        v: &Verification,
    ) -> Result<Evidence, Error> {
        let p = project(r, t)?;
        compare_configuration(&p, c)?;
        // Project settings apply to future builds; the live production revision
        // proves continuity, not that an existing deployment rebuilt itself.
        let id = production_id(&p)?;
        let row = r.bearer_json(&t.token_secret, "GET", deployment_url(t, id), Value::Null)?;
        let revision = row
            .pointer("/gitSource/sha")
            .or_else(|| row.pointer("/meta/githubCommitSha"))
            .and_then(Value::as_str)
            .ok_or(Error::Response("Vercel current revision"))?;
        check_deployment(t, &row, DeploymentTarget::Production, revision)?;
        if required(&row, "id")? != id {
            return Err(Error::Binding("Vercel current deployment differs"));
        }
        let mut evidence =
            verify_deployment(r, t, DeploymentTarget::Production, i, v, &row, revision)?;
        evidence.provider_ref = project_url(t);
        Ok(evidence)
    }
}
fn compare_configuration(row: &Value, c: &ConfigurationChange) -> Result<(), Error> {
    for (key, wanted) in c.body().as_object().expect("constructed object") {
        let actual = row
            .get(key)
            .ok_or(Error::Response("Vercel project configuration"))?;
        if !actual.is_string() && !actual.is_null() {
            return Err(Error::Response("Vercel project configuration"));
        }
        if actual != wanted {
            return Err(Error::Pending("Vercel project configuration differs"));
        }
    }
    Ok(())
}

impl ResourceAction for Rollback {
    type Target = Target;
    type Change = RollbackChange;
    const ENTITY_TYPE: &'static str = "DsfVercelProject";
    const ENTITY_SET: &'static str = "DsfVercelProjects";
    const ACTION: &'static str = "Rollback";
    const RESULT: VerifiedValue = VerifiedValue::Revision;
    fn validate_target(t: &Target, r: &Value) -> Result<(), Error> {
        validate_target(t, r)
    }
    fn validate_change(_: &Target, c: &RollbackChange, i: &Invocation) -> Result<(), Error> {
        validate_scope(c.target, i)?;
        identifier(&c.deployment_id)?;
        identifier(&c.baseline_deployment_id)?;
        if c.target != DeploymentTarget::Production
            || !full_sha(&i.revision)
            || required(&i.resource, "rollback_execution_id")? != c.deployment_id
            || c.deployment_id == c.baseline_deployment_id
        {
            return Err(Error::Binding(
                "rollback must select a previous production deployment",
            ));
        }
        Ok(())
    }
    fn execute(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &RollbackChange,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        let p = project(r, t)?;
        let current = production_id(&p)?;
        let row = deployment(r, t, &c.deployment_id, c.target, &i.revision)?;
        ready(&row)?;
        if current == c.deployment_id {
            return Ok(receipt(t, &c.deployment_id));
        }
        if current != c.baseline_deployment_id {
            return Err(Error::Binding("production changed before rollback"));
        }
        if i.execution_attempts != 1 {
            return Err(Error::Pending("rollback outcome requires reconciliation"));
        }
        let secret = r.credential(&t.token_secret)?;
        let response = r.host.request(&Request {
            method: "POST",
            url: format!(
                "https://api.vercel.com/v1/projects/{}/rollback/{}?teamId={}",
                encoded(&t.project_id),
                encoded(&c.deployment_id),
                encoded(&t.account_id)
            ),
            headers: vec![("authorization".into(), format!("Bearer {secret}"))],
            body: String::new(),
        })?;
        if response.status != 201 {
            return Err(Error::Http(response.status, "Vercel rollback"));
        }
        Ok(receipt(t, &c.deployment_id))
    }
    fn observe(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &RollbackChange,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        let p = project(r, t)?;
        if production_id(&p)? != c.deployment_id {
            return Err(Error::Pending(
                "rollback has not changed production pointer",
            ));
        }
        ready(&deployment(r, t, &c.deployment_id, c.target, &i.revision)?)?;
        Ok(receipt(t, &c.deployment_id))
    }
    fn verify(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &RollbackChange,
        i: &Invocation,
        v: &Verification,
    ) -> Result<Evidence, Error> {
        let p = project(r, t)?;
        if production_id(&p)? != c.deployment_id {
            return Err(Error::Pending("rollback production pointer differs"));
        }
        let row = deployment(r, t, &c.deployment_id, c.target, &i.revision)?;
        verify_deployment(r, t, c.target, i, v, &row, &i.revision)
    }
}
fn alias_url(t: &Target, alias: &str) -> String {
    format!(
        "https://api.vercel.com/v4/aliases/{}?projectId={}&teamId={}",
        encoded(alias),
        encoded(&t.project_id),
        encoded(&t.account_id)
    )
}
fn alias_matches(r: &mut Runtime<impl Host>, t: &Target, c: &AliasChange) -> Result<bool, Error> {
    let row = match r.bearer_json(&t.token_secret, "GET", alias_url(t, &c.alias), Value::Null) {
        Ok(row) => row,
        Err(Error::Http(404, _)) => return Ok(false),
        Err(e) => return Err(e),
    };
    if required(&row, "alias")? != c.alias || required(&row, "projectId")? != t.project_id {
        return Err(Error::Binding("alias belongs to another project"));
    }
    if row.get("redirect").is_some_and(|v| !v.is_null()) {
        return Err(Error::Binding("alias is a redirect"));
    }
    let id = row
        .get("deploymentId")
        .ok_or(Error::Response("Vercel alias deployment"))?;
    if !id.is_string() && !id.is_null() {
        return Err(Error::Response("Vercel alias deployment"));
    }
    Ok(id.as_str() == Some(&c.deployment_id))
}
impl ResourceAction for SetAlias {
    type Target = Target;
    type Change = AliasChange;
    const ENTITY_TYPE: &'static str = "DsfVercelProject";
    const ENTITY_SET: &'static str = "DsfVercelProjects";
    const ACTION: &'static str = "SetAlias";
    const RESULT: VerifiedValue = VerifiedValue::Configuration;
    fn validate_target(t: &Target, r: &Value) -> Result<(), Error> {
        validate_target(t, r)
    }
    fn validate_change(t: &Target, c: &AliasChange, i: &Invocation) -> Result<(), Error> {
        validate_scope(c.target, i)?;
        identifier(&c.deployment_id)?;
        if !hostname(&c.alias)
            || !t.allowed_aliases.contains(&c.alias)
            || required(&i.resource, "alias")? != c.alias
            || required(&i.resource, "provider_execution_id")? != c.deployment_id
            || !full_sha(&c.revision)
        {
            return Err(Error::Binding(
                "alias or deployment differs from accepted action",
            ));
        }
        Ok(())
    }
    fn execute(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &AliasChange,
        i: &Invocation,
    ) -> Result<Receipt, Error> {
        project(r, t)?;
        ready(&deployment(r, t, &c.deployment_id, c.target, &c.revision)?)?;
        if alias_matches(r, t, c)? {
            return Ok(receipt(t, &c.deployment_id));
        }
        // A lost alias POST is read back, never repeated against a newer owner.
        if i.execution_attempts != 1 {
            return Err(Error::Pending("alias assignment requires reconciliation"));
        }
        let row = r.bearer_json(
            &t.token_secret,
            "POST",
            format!(
                "https://api.vercel.com/v2/deployments/{}/aliases?teamId={}",
                encoded(&c.deployment_id),
                encoded(&t.account_id)
            ),
            json!({"alias":c.alias}),
        )?;
        if required(&row, "alias")? != c.alias {
            return Err(Error::Response("Vercel alias assignment"));
        }
        Ok(receipt(t, &c.deployment_id))
    }
    fn observe(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &AliasChange,
        _: &Invocation,
    ) -> Result<Receipt, Error> {
        project(r, t)?;
        ready(&deployment(r, t, &c.deployment_id, c.target, &c.revision)?)?;
        if !alias_matches(r, t, c)? {
            return Err(Error::Pending("alias has not reached selected deployment"));
        }
        Ok(receipt(t, &c.deployment_id))
    }
    fn verify(
        r: &mut Runtime<impl Host>,
        t: &Target,
        c: &AliasChange,
        i: &Invocation,
        v: &Verification,
    ) -> Result<Evidence, Error> {
        project(r, t)?;
        let row = deployment(r, t, &c.deployment_id, c.target, &c.revision)?;
        if !alias_matches(r, t, c)? {
            return Err(Error::Pending("alias points elsewhere"));
        }
        ready(&row)?;
        let (flow_ref, telemetry_ref, observed_revision) = verify_vercel_alias(
            r,
            v,
            i,
            &format!("https://{}", c.alias),
            &t.allowed_aliases,
            &c.revision,
        )?;
        Ok(Evidence {
            provider_ref: alias_url(t, &c.alias),
            flow_ref,
            telemetry_ref,
            observed_revision,
            observed_configuration: i.configuration.clone(),
        })
    }
}
#[cfg(test)]
mod tests;
