//! temper_deploy_lifecycle — one side effect per trigger for TemperDeploy.
//!
//! | trigger        | side effect                                      | reports |
//! |----------------|--------------------------------------------------|---------|
//! | Request / CheckImage | one GHCR manifest probe for image_tag      | ImageReady / ImagePending / Fail |
//! | ImageReady     | read IMAGE_TAG, upsert new tag, redeploy         | SwapSucceeded |
//! | Check          | one /healthz + /paw/version probe                | CheckHealthy / CheckPending / CheckUnhealthy |
//! | CheckUnhealthy | restore previous_tag, redeploy                   | RollbackPushed |
//!
//! Railway ids and URLs come from trigger config (trusted). image_tag and
//! expected_sha come from the row. Does not dispatch Effort.

use temper_wasm_sdk::prelude::*;

const MAX_CHECKS_CEILING: u64 = 240;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let run = DeployFields::from_state(&ctx, &fields)?;
        ctx.log(
            "info",
            &format!(
                "temper_deploy_lifecycle: {} on {} tag {} sha {}",
                ctx.trigger_action, ctx.entity_id, run.image_tag, run.expected_sha
            ),
        );
        let (action, params) = match ctx.trigger_action.as_str() {
            "Request" | "CheckImage" => wait_image(&ctx, &run)?,
            "ImageReady" => swap(&ctx, &run)?,
            "Check" => poll(&ctx, &run)?,
            "CheckUnhealthy" => rollback(&ctx, &run)?,
            other => {
                return Err(format!(
                    "temper_deploy_lifecycle: unsupported trigger {other}"
                ));
            }
        };
        set_success_result(action, &params);
        Ok(())
    })();
    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

struct DeployFields {
    image_tag: String,
    expected_sha: String,
    previous_tag: String,
    max_checks: u64,
    check_count: u64,
    image_check_count: u64,
}

impl DeployFields {
    fn from_state(ctx: &Context, fields: &Value) -> Result<Self, String> {
        Ok(Self {
            image_tag: required(ctx, fields, "image_tag")?,
            expected_sha: required(ctx, fields, "expected_sha")?,
            previous_tag: param_or_field(ctx, fields, "previous_tag").unwrap_or_default(),
            max_checks: parse_max_checks(
                &param_or_field(ctx, fields, "max_checks").unwrap_or_else(|| "60".into()),
            )?,
            check_count: counter_field(fields, "check_count"),
            image_check_count: counter_field(fields, "image_check_count"),
        })
    }
}

fn wait_image(ctx: &Context, run: &DeployFields) -> Result<(&'static str, Value), String> {
    validate_image_tag(&run.image_tag)?;
    validate_sha(&run.expected_sha)?;
    if run.image_check_count >= run.max_checks {
        return Err(format!(
            "temper_deploy_lifecycle: GHCR still missing {} after {} checks",
            run.image_tag, run.image_check_count
        ));
    }
    if ghcr_has_tag(ctx, &run.image_tag)? {
        Ok(("ImageReady", json!({})))
    } else {
        Ok(("ImagePending", json!({})))
    }
}

fn ghcr_has_tag(ctx: &Context, image_tag: &str) -> Result<bool, String> {
    let github_token = ctx
        .config
        .get("github_token")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .cloned()
        .ok_or_else(|| "temper_deploy_lifecycle: missing config github_token".to_string())?;
    let name = ctx
        .config
        .get("ghcr_name")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "nerdsane/temperpaw".into());
    let pull_token = ghcr_pull_token(ctx, &name, &github_token)?;
    let url = ghcr_manifest_url(&name, image_tag)?;
    let headers = vec![
        (
            "accept".into(),
            "application/vnd.oci.image.index.v1+json".into(),
        ),
        ("authorization".into(), format!("Bearer {pull_token}")),
    ];
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    match resp.status {
        200 => Ok(true),
        404 => Ok(false),
        other => Err(format!(
            "temper_deploy_lifecycle: GHCR manifest HTTP {other} for {image_tag}"
        )),
    }
}

fn ghcr_pull_token(ctx: &Context, name: &str, github_token: &str) -> Result<String, String> {
    let url = ghcr_token_url(name)?;
    let headers = vec![
        ("accept".into(), "application/json".into()),
        ("authorization".into(), format!("Bearer {github_token}")),
    ];
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status != 200 {
        return Err(format!(
            "temper_deploy_lifecycle: GHCR token HTTP {} for {name}",
            resp.status
        ));
    }
    let v: Value = serde_json::from_str(&resp.body)
        .map_err(|_| "temper_deploy_lifecycle: GHCR token body is not JSON".to_string())?;
    v.get("token")
        .or_else(|| v.get("access_token"))
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "temper_deploy_lifecycle: GHCR token body has no token".into())
}

fn ghcr_token_url(name: &str) -> Result<String, String> {
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '_')
        || !name.contains('/')
    {
        return Err(format!(
            "temper_deploy_lifecycle: ghcr_name must be owner/name, got {name:?}"
        ));
    }
    Ok(format!(
        "https://ghcr.io/token?service=ghcr.io&scope=repository:{name}:pull"
    ))
}

fn ghcr_manifest_url(name: &str, image_tag: &str) -> Result<String, String> {
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-' || c == '_')
        || !name.contains('/')
    {
        return Err(format!(
            "temper_deploy_lifecycle: ghcr_name must be owner/name, got {name:?}"
        ));
    }
    validate_image_tag(image_tag)?;
    Ok(format!("https://ghcr.io/v2/{name}/manifests/{image_tag}"))
}

fn swap(ctx: &Context, run: &DeployFields) -> Result<(&'static str, Value), String> {
    validate_image_tag(&run.image_tag)?;
    validate_sha(&run.expected_sha)?;
    let cfg = railway_config(ctx)?;
    let previous_tag = railway_image_tag(ctx, &cfg)?;
    let previous_sha = live_sha(ctx, &cfg).unwrap_or_default();
    upsert_image_tag(ctx, &cfg, &run.image_tag)?;
    let dep = latest_deployment_id(ctx, &cfg)?;
    redeploy(ctx, &cfg, &dep)?;
    Ok((
        "SwapSucceeded",
        json!({
            "previous_tag": previous_tag,
            "previous_sha": previous_sha,
        }),
    ))
}

fn poll(ctx: &Context, run: &DeployFields) -> Result<(&'static str, Value), String> {
    validate_sha(&run.expected_sha)?;
    let cfg = railway_config(ctx)?;
    if run.check_count >= run.max_checks {
        return Ok((
            "CheckUnhealthy",
            json!({
                "reason": format!("max_checks {} spent without expected sha", run.max_checks),
                "observed_sha": live_sha(ctx, &cfg).unwrap_or_default(),
            }),
        ));
    }
    if !readyz_ok(ctx, &cfg) {
        return Ok(("CheckPending", json!({ "observed_sha": "" })));
    }
    let observed = live_sha(ctx, &cfg).unwrap_or_default();
    if observed == run.expected_sha {
        Ok(("CheckHealthy", json!({ "observed_sha": observed })))
    } else {
        Ok(("CheckPending", json!({ "observed_sha": observed })))
    }
}

fn rollback(ctx: &Context, run: &DeployFields) -> Result<(&'static str, Value), String> {
    if run.previous_tag.is_empty() {
        return Err("temper_deploy_lifecycle: empty previous_tag; cannot roll back".into());
    }
    validate_image_tag(&run.previous_tag)?;
    let cfg = railway_config(ctx)?;
    upsert_image_tag(ctx, &cfg, &run.previous_tag)?;
    let dep = latest_deployment_id(ctx, &cfg)?;
    redeploy(ctx, &cfg, &dep)?;
    Ok((
        "RollbackPushed",
        json!({ "previous_tag": run.previous_tag }),
    ))
}

struct RailwayConfig {
    graphql_url: String,
    token: String,
    project_id: String,
    environment_id: String,
    service_id: String,
    base_url: String,
    version_path: String,
    ready_path: String,
    version_field: String,
    temper_api_key: String,
}

fn railway_config(ctx: &Context) -> Result<RailwayConfig, String> {
    let cfg = |key: &str| {
        ctx.config
            .get(key)
            .filter(|v| !v.is_empty() && !v.contains("{secret:"))
            .cloned()
            .ok_or_else(|| format!("temper_deploy_lifecycle: missing config {key}"))
    };
    Ok(RailwayConfig {
        graphql_url: ctx
            .config
            .get("railway_graphql_url")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "https://backboard.railway.com/graphql/v2".into()),
        token: cfg("railway_token")?,
        project_id: cfg("railway_project_id")?,
        environment_id: cfg("railway_environment_id")?,
        service_id: cfg("railway_service_id")?,
        base_url: cfg("base_url")?.trim_end_matches('/').to_string(),
        version_path: ctx
            .config
            .get("version_path")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "/paw/version".into()),
        ready_path: ctx
            .config
            .get("ready_path")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "/healthz".into()),
        version_field: ctx
            .config
            .get("version_field")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| "sha".into()),
        temper_api_key: ctx
            .config
            .get("temper_api_key")
            .filter(|v| !v.is_empty() && !v.contains("{secret:"))
            .cloned()
            .unwrap_or_default(),
    })
}

fn railway_headers(cfg: &RailwayConfig) -> Vec<(String, String)> {
    vec![
        ("content-type".into(), "application/json".into()),
        ("authorization".into(), format!("Bearer {}", cfg.token)),
    ]
}

fn gql(ctx: &Context, cfg: &RailwayConfig, body: &str) -> Result<Value, String> {
    let resp = ctx.http_call("POST", &cfg.graphql_url, &railway_headers(cfg), body)?;
    if resp.status >= 400 {
        return Err(format!(
            "temper_deploy_lifecycle: Railway GraphQL HTTP {}",
            resp.status
        ));
    }
    let v: Value = serde_json::from_str(&resp.body)
        .map_err(|e| format!("temper_deploy_lifecycle: Railway body: {e}"))?;
    if v.get("errors").is_some() {
        let msg = v
            .pointer("/errors/0/message")
            .and_then(|m| m.as_str())
            .unwrap_or("Railway GraphQL error");
        return Err(format!("temper_deploy_lifecycle: {msg}"));
    }
    Ok(v)
}

fn railway_image_tag(ctx: &Context, cfg: &RailwayConfig) -> Result<String, String> {
    let body = variables_query(&cfg.project_id, &cfg.environment_id, &cfg.service_id);
    let v = gql(ctx, cfg, &body)?;
    Ok(v.pointer("/data/variables/IMAGE_TAG")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string())
}

fn upsert_image_tag(ctx: &Context, cfg: &RailwayConfig, tag: &str) -> Result<(), String> {
    let body = variable_upsert_body(
        &cfg.project_id,
        &cfg.environment_id,
        &cfg.service_id,
        "IMAGE_TAG",
        tag,
    );
    gql(ctx, cfg, &body)?;
    Ok(())
}

fn latest_deployment_id(ctx: &Context, cfg: &RailwayConfig) -> Result<String, String> {
    let body = latest_deployment_query(&cfg.service_id);
    let v = gql(ctx, cfg, &body)?;
    let edges = v
        .pointer("/data/service/serviceInstances/edges")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    for edge in edges {
        let env = edge
            .pointer("/node/environmentId")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if env != cfg.environment_id {
            continue;
        }
        if let Some(id) = edge
            .pointer("/node/latestDeployment/id")
            .and_then(|x| x.as_str())
        {
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }
    Err("temper_deploy_lifecycle: no latest deployment id".into())
}

fn redeploy(ctx: &Context, cfg: &RailwayConfig, deployment_id: &str) -> Result<(), String> {
    let body = redeploy_body(deployment_id);
    gql(ctx, cfg, &body)?;
    Ok(())
}

fn readyz_ok(ctx: &Context, cfg: &RailwayConfig) -> bool {
    let url = format!("{}{}", cfg.base_url, cfg.ready_path);
    match ctx.http_call("GET", &url, &[], "") {
        Ok(resp) => resp.status < 400,
        Err(_) => false,
    }
}

fn live_sha(ctx: &Context, cfg: &RailwayConfig) -> Result<String, String> {
    let url = format!("{}{}", cfg.base_url, cfg.version_path);
    let mut headers = vec![];
    if !cfg.temper_api_key.is_empty() {
        headers.push((
            "authorization".into(),
            format!("Bearer {}", cfg.temper_api_key),
        ));
    }
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "temper_deploy_lifecycle: version HTTP {}",
            resp.status
        ));
    }
    parse_version_sha(&resp.body, &cfg.version_field)
        .ok_or_else(|| "temper_deploy_lifecycle: version JSON missing sha".into())
}

fn required(ctx: &Context, fields: &Value, key: &str) -> Result<String, String> {
    param_or_field(ctx, fields, key)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("temper_deploy_lifecycle: missing {key}"))
}

fn param_or_field(ctx: &Context, fields: &Value, key: &str) -> Option<String> {
    ctx.trigger_params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            fields
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            fields
                .get(&pascal(key))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
}

fn counter_field(fields: &Value, key: &str) -> u64 {
    fields
        .get(key)
        .or_else(|| fields.get(&pascal(key)))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn pascal(field: &str) -> String {
    field
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn parse_max_checks(raw: &str) -> Result<u64, String> {
    let n: u64 = raw.trim().parse().map_err(|_| {
        format!("temper_deploy_lifecycle: max_checks must be a number, got {raw:?}")
    })?;
    if n == 0 {
        return Err("temper_deploy_lifecycle: max_checks must be > 0".into());
    }
    Ok(n.min(MAX_CHECKS_CEILING))
}

fn validate_image_tag(tag: &str) -> Result<(), String> {
    if tag == "edge" || tag == "latest" {
        return Ok(());
    }
    if let Some(rest) = tag.strip_prefix("sha-") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(());
        }
    }
    Err(format!(
        "temper_deploy_lifecycle: IMAGE_TAG must be edge, latest, or sha-<hex>, got {tag:?}"
    ))
}

fn validate_sha(sha: &str) -> Result<(), String> {
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(format!(
        "temper_deploy_lifecycle: expected_sha must be 40 hex, got {sha:?}"
    ))
}

fn parse_version_sha(body: &str, field: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get(field)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn variables_query(project_id: &str, environment_id: &str, service_id: &str) -> String {
    format!(
        r#"{{"query":"query($p:String!,$e:String!,$s:String!){{variables(projectId:$p,environmentId:$e,serviceId:$s)}}","variables":{{"p":"{}","e":"{}","s":"{}"}}}}"#,
        escape_json(project_id),
        escape_json(environment_id),
        escape_json(service_id)
    )
}

fn variable_upsert_body(
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    name: &str,
    value: &str,
) -> String {
    format!(
        r#"{{"query":"mutation($input:VariableUpsertInput!){{variableUpsert(input:$input)}}","variables":{{"input":{{"projectId":"{}","environmentId":"{}","serviceId":"{}","name":"{}","value":"{}","skipDeploys":true}}}}}}"#,
        escape_json(project_id),
        escape_json(environment_id),
        escape_json(service_id),
        escape_json(name),
        escape_json(value)
    )
}

fn latest_deployment_query(service_id: &str) -> String {
    format!(
        r#"{{"query":"query($s:String!){{service(id:$s){{serviceInstances{{edges{{node{{environmentId latestDeployment{{id}}}}}}}}}}}}","variables":{{"s":"{}"}}}}"#,
        escape_json(service_id)
    )
}

fn redeploy_body(deployment_id: &str) -> String {
    format!(
        r#"{{"query":"mutation($d:String!){{deploymentRedeploy(id:$d){{id status}}}}","variables":{{"d":"{}"}}}}"#,
        escape_json(deployment_id)
    )
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tag_accepts_edge_latest_sha() {
        assert!(validate_image_tag("edge").is_ok());
        assert!(validate_image_tag("latest").is_ok());
        assert!(validate_image_tag("sha-deadbeef").is_ok());
        assert!(validate_image_tag("v1").is_err());
        assert!(validate_image_tag("sha-").is_err());
        assert!(validate_image_tag("sha-zz").is_err());
    }

    #[test]
    fn sha_is_strict_40_hex() {
        let ok = "0123456789abcdef0123456789abcdef01234567";
        assert!(validate_sha(ok).is_ok());
        assert!(validate_sha("abc").is_err());
        assert!(validate_sha("").is_err());
    }

    #[test]
    fn version_json_reads_named_field() {
        assert_eq!(
            parse_version_sha(r#"{"sha":"abc"}"#, "sha").as_deref(),
            Some("abc")
        );
        assert_eq!(
            parse_version_sha(r#"{"commit":"xyz"}"#, "commit").as_deref(),
            Some("xyz")
        );
        assert_eq!(parse_version_sha(r#"{"sha":""}"#, "sha"), None);
    }

    #[test]
    fn max_checks_bounded() {
        assert_eq!(parse_max_checks("60").unwrap(), 60);
        assert_eq!(parse_max_checks("999").unwrap(), MAX_CHECKS_CEILING);
        assert!(parse_max_checks("0").is_err());
        assert!(parse_max_checks("nope").is_err());
    }

    #[test]
    fn ghcr_url_is_owner_name_and_tag() {
        assert_eq!(
            ghcr_manifest_url("nerdsane/temperpaw", "sha-deadbeef").unwrap(),
            "https://ghcr.io/v2/nerdsane/temperpaw/manifests/sha-deadbeef"
        );
        assert_eq!(
            ghcr_token_url("nerdsane/temperpaw").unwrap(),
            "https://ghcr.io/token?service=ghcr.io&scope=repository:nerdsane/temperpaw:pull"
        );
        assert!(ghcr_manifest_url("no-slash", "sha-ab").is_err());
        assert!(ghcr_token_url("no-slash").is_err());
        assert!(ghcr_manifest_url("nerdsane/temperpaw", "v1").is_err());
    }

    #[test]
    fn graphql_bodies_do_not_embed_raw_quotes() {
        let q = variables_query(r#"p"x"#, "e", "s");
        assert!(q.contains(r#"p\"x"#), "{q}");
        let u = variable_upsert_body("p", "e", "s", "IMAGE_TAG", "sha-ab");
        assert!(u.contains("skipDeploys"));
        assert!(u.contains("sha-ab"));
    }
}
