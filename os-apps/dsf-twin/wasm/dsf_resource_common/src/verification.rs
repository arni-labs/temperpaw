use crate::*;
use sha2::{Digest, Sha256};

pub const DSF_API: &str = "https://api.deep-sci-fi.world";
pub const DSF_WEB: &str = "https://deep-sci-fi.world";

/// The provider adapter must first read and compare every requested setting.
pub fn verify_configuration(
    runtime: &mut Runtime<impl Host>,
    verification: &Verification,
    invocation: &Invocation,
    provider_ref: String,
) -> Result<Evidence, Error> {
    let origin = railway_application_origin(runtime, &verification.application, None)?;
    let (flow_ref, telemetry_ref, revision) =
        verify_product_at(runtime, verification, invocation, &origin, None)?;
    Ok(Evidence {
        provider_ref,
        flow_ref,
        telemetry_ref,
        observed_revision: revision,
        observed_configuration: invocation.configuration.clone(),
    })
}

/// Check the product response and the actual Datadog trace for that request.
pub fn verify_product(
    runtime: &mut Runtime<impl Host>,
    verification: &Verification,
    invocation: &Invocation,
    origin: &str,
    expected_revision: Option<&str>,
) -> Result<(String, String, String), Error> {
    let bound = railway_application_origin(runtime, &verification.application, None)?;
    if bound != origin.trim_end_matches('/') {
        return Err(Error::Binding(
            "requested probe origin differs from application binding",
        ));
    }
    verify_product_at(runtime, verification, invocation, &bound, expected_revision)
}

/// The caller must first bind this URL to the exact provider project and revision.
pub fn verify_vercel_preview(
    runtime: &mut Runtime<impl Host>,
    verification: &Verification,
    invocation: &Invocation,
    origin: &str,
    expected_revision: &str,
) -> Result<(String, String, String), Error> {
    verification.application.vercel(&invocation.resource_id)?;
    let parsed =
        url::Url::parse(origin).map_err(|_| Error::Binding("invalid Vercel preview URL"))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
        || !parsed
            .host_str()
            .is_some_and(|host| host.ends_with(".vercel.app"))
    {
        return Err(Error::Binding(
            "Vercel preview URL is not a deployment origin",
        ));
    }
    if !matches!(
        verification.flow,
        Flow::WebPage { .. } | Flow::ProviderConfiguration {}
    ) {
        return Err(Error::Binding(
            "preview verification requires a flow on that preview",
        ));
    }
    verify_product_at(
        runtime,
        verification,
        invocation,
        parsed.as_str().trim_end_matches('/'),
        Some(expected_revision),
    )
}

/// The adapter must first prove alias ownership and its exact deployment target.
pub fn verify_vercel_alias(
    runtime: &mut Runtime<impl Host>,
    verification: &Verification,
    invocation: &Invocation,
    origin: &str,
    allowed_aliases: &[String],
    expected_revision: &str,
) -> Result<(String, String, String), Error> {
    verification.application.vercel(&invocation.resource_id)?;
    let parsed = url::Url::parse(origin).map_err(|_| Error::Binding("invalid Vercel alias URL"))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
        || !parsed
            .host_str()
            .is_some_and(|host| allowed_aliases.iter().any(|alias| alias == host))
    {
        return Err(Error::Binding(
            "Vercel alias is not registered on this resource",
        ));
    }
    if !matches!(
        verification.flow,
        Flow::WebPage { .. } | Flow::ProviderConfiguration {}
    ) {
        return Err(Error::Binding(
            "alias verification requires a flow on that alias",
        ));
    }
    verify_product_at(
        runtime,
        verification,
        invocation,
        parsed.as_str().trim_end_matches('/'),
        Some(expected_revision),
    )
}

fn verify_product_at(
    runtime: &mut Runtime<impl Host>,
    verification: &Verification,
    invocation: &Invocation,
    origin: &str,
    expected_revision: Option<&str>,
) -> Result<(String, String, String), Error> {
    let request_id = format!(
        "dsf-{:x}",
        Sha256::digest(
            format!(
                "{}:{}:{}",
                invocation.resource_id, invocation.operation_key, invocation.sequence
            )
            .as_bytes()
        )
    );
    let health_url = format!("{origin}/api/health");
    let health = probe(runtime, &health_url, &request_id, None)?;
    let revision = required(&health, "git_sha")?.to_owned();
    if required(&health, "status")? != "healthy"
        || !full_sha(&revision)
        || expected_revision.is_some_and(|expected| expected != revision)
    {
        return Err(Error::Pending(
            "health response is not healthy at the expected source revision",
        ));
    }
    let flow_ref = match &verification.flow {
        Flow::WebPage {
            path,
            required_text,
        } => {
            if required_text.trim().is_empty() || required_text.len() > 512 {
                return Err(Error::Binding("web flow requires bounded expected content"));
            }
            let base =
                url::Url::parse(origin).map_err(|_| Error::Binding("invalid web flow origin"))?;
            let url = base
                .join(path)
                .map_err(|_| Error::Binding("invalid web flow path"))?;
            if url.origin() != base.origin()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(Error::Binding("web flow escaped its provider origin"));
            }
            let response = runtime.host.request(&Request {
                method: "GET",
                url: url.to_string(),
                headers: vec![
                    ("accept".into(), "text/html".into()),
                    ("x-request-id".into(), request_id.clone()),
                    ("cache-control".into(), "no-cache".into()),
                ],
                body: String::new(),
            })?;
            if response.status != 200
                || response.body.len() > 1_048_576
                || !response.body.contains(required_text)
            {
                return Err(Error::Pending("web flow does not show expected content"));
            }
            url.to_string()
        }
        Flow::ProviderConfiguration {} => health_url,
        Flow::Story { story_id, world_id } => {
            identifier(story_id)?;
            identifier(world_id)?;
            let url = format!("{origin}/api/stories/{}", encoded(story_id));
            let value = probe(runtime, &url, &request_id, None)?;
            let story = value.get("story").ok_or(Error::Response("story read"))?;
            if required(story, "id")? != story_id
                || required(story, "world_id")? != world_id
                || !["published", "acclaimed"].contains(&required(story, "status")?)
                || required(story, "content")?.is_empty()
            {
                return Err(Error::Pending("selected story is not readable"));
            }
            url
        }
        Flow::OperationalSnapshot {
            schema_version,
            secret_name,
        } => {
            if origin != DSF_API {
                return Err(Error::Binding(
                    "operational snapshot admin credential is production-only",
                ));
            }
            let url = format!("{origin}/api/operations/snapshot?participant_limit=1&job_limit=1");
            let value = probe(runtime, &url, &request_id, Some(secret_name))?;
            if value.get("snapshot_version") != Some(&json!(1))
                || value.get("revision").and_then(Value::as_str) != Some(&revision)
                || value.pointer("/schema/is_current") != Some(&json!(true))
                || value
                    .pointer("/schema/current_version")
                    .and_then(Value::as_str)
                    != Some(schema_version)
            {
                return Err(Error::Pending("operational snapshot schema differs"));
            }
            url
        }
        Flow::Media {} => health_url,
    };
    let telemetry_ref = verify_datadog(
        runtime,
        &verification.datadog,
        &request_id,
        &revision,
        &format!("{origin}/api/health"),
    )?;
    Ok((flow_ref, telemetry_ref, revision))
}

pub fn probe(
    runtime: &mut Runtime<impl Host>,
    url: &str,
    request_id: &str,
    secret: Option<&str>,
) -> Result<Value, Error> {
    let mut headers = vec![
        ("x-request-id".into(), request_id.into()),
        ("accept".into(), "application/json".into()),
        ("cache-control".into(), "no-cache".into()),
    ];
    if let Some(name) = secret {
        headers.push((
            "authorization".into(),
            format!("Bearer {}", runtime.credential(name)?),
        ));
    }
    json_body(
        runtime.host.request(&Request {
            method: "GET",
            url: url.into(),
            headers,
            body: String::new(),
        })?,
        "DSF probe",
    )
}

pub fn datadog_site(site: &str) -> Result<&str, Error> {
    if [
        "datadoghq.com",
        "us3.datadoghq.com",
        "us5.datadoghq.com",
        "datadoghq.eu",
        "ap1.datadoghq.com",
        "ap2.datadoghq.com",
        "uk1.datadoghq.com",
    ]
    .contains(&site)
    {
        Ok(site)
    } else {
        Err(Error::Binding("Datadog site is not permitted"))
    }
}

pub fn verify_datadog(
    runtime: &mut Runtime<impl Host>,
    dd: &Datadog,
    request_id: &str,
    revision: &str,
    health_url: &str,
) -> Result<String, Error> {
    datadog_site(&dd.site)?;
    identifier(&dd.service)?;
    identifier(&dd.environment)?;
    identifier(request_id)?;
    if !full_sha(revision) {
        return Err(Error::Binding("invalid probe revision"));
    }
    let query = format!(
        "service:{} env:{} @git.commit.sha:{} @dsf.request_id:{} -status:error",
        dd.service, dd.environment, revision, request_id
    );
    let body = json!({"data":{"type":"search_request","attributes":{"filter":{
        "from":(runtime.now_ms-1_800_000).to_string(), "to":runtime.now_ms.to_string(), "query":query},
        "page":{"limit":20},"sort":"-timestamp"}}});
    let api_key = runtime.credential(&dd.api_key_secret)?;
    let app_key = runtime.credential(&dd.app_key_secret)?;
    let value = json_body(
        runtime.host.request(&Request {
            method: "POST",
            url: format!("https://api.{}/api/v2/spans/events/search", dd.site),
            headers: vec![
                ("DD-API-KEY".into(), api_key),
                ("DD-APPLICATION-KEY".into(), app_key),
                ("content-type".into(), "application/json".into()),
            ],
            body: body.to_string(),
        })?,
        "Datadog",
    )?;
    let spans = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or(Error::Response("Datadog spans"))?;
    let span = spans
        .iter()
        .take(20)
        .find(|span| matching_span(span, dd, request_id, revision, health_url))
        .ok_or(Error::Pending(
            "Datadog has not indexed the exact successful probe",
        ))?;
    let trace_id = required(
        span.get("attributes")
            .ok_or(Error::Response("Datadog span"))?,
        "trace_id",
    )?;
    Ok(format!(
        "https://app.{}/apm/trace/{}",
        dd.site,
        encoded(trace_id)
    ))
}

fn matching_span(
    span: &Value,
    dd: &Datadog,
    request_id: &str,
    revision: &str,
    health_url: &str,
) -> bool {
    let Some(attrs) = span.get("attributes") else {
        return false;
    };
    attrs
        .pointer("/custom/http/url")
        .and_then(Value::as_str)
        .is_some_and(|observed| same_probe_url(observed, health_url))
        && attrs.get("service").and_then(Value::as_str) == Some(&dd.service)
        && attrs.get("env").and_then(Value::as_str) == Some(&dd.environment)
        && attrs.get("status").and_then(Value::as_str) == Some("ok")
        && attrs
            .pointer("/custom/http/status_code")
            .is_some_and(|code| code.as_u64() == Some(200) || code.as_str() == Some("200"))
        && attrs
            .pointer("/custom/git/commit/sha")
            .and_then(Value::as_str)
            == Some(revision)
        && attrs
            .pointer("/custom/dsf/request_id")
            .and_then(Value::as_str)
            == Some(request_id)
}

fn same_probe_url(observed: &str, expected: &str) -> bool {
    // The bound external probe uses HTTPS; TLS termination can leave HTTP in its span.
    // Compare the remaining bytes exactly: do not normalize host, path, ports or credentials.
    observed == expected
        || expected
            .strip_prefix("https://")
            .is_some_and(|suffix| observed.strip_prefix("http://") == Some(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn railway_tls_termination_preserves_the_exact_probe_identity() {
        let span: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/railway_proxy_health_span.json"
        ))
        .unwrap();
        let dd = Datadog {
            site: "datadoghq.com".into(),
            service: "deep-sci-fi-backend".into(),
            environment: "staging".into(),
            api_key_secret: "dd-api".into(),
            app_key_secret: "dd-app".into(),
        };
        let request_id = "dsf-fd4862bc62da72b986b4d8bee2d4c05cd75d567a091236b8885e83fb674acf17";
        let revision = "7597673d083654bad15596116d2b4893b085e0c0";
        let expected = "https://deep-sci-fi-staging.up.railway.app/api/health";
        assert!(matching_span(&span, &dd, request_id, revision, expected));
        for observed in [
            "http://api.deep-sci-fi.world/api/health",
            "http://deep-sci-fi-staging.up.railway.app.attacker.test/api/health",
            "http://deep-sci-fi-staging.up.railway.app/api/worlds",
            "http://deep-sci-fi-staging.up.railway.app/api/health?probe=other",
            "http://deep-sci-fi-staging.up.railway.app/api/health#fragment",
            "http://deep-sci-fi-staging.up.railway.app:80/api/health",
            "http://deep-sci-fi-staging.up.railway.app:443/api/health",
            "http://user:password@deep-sci-fi-staging.up.railway.app/api/health",
            "http://deep-sci-fi-staging.up.railway.app/api/%68ealth",
            "//deep-sci-fi-staging.up.railway.app/api/health",
        ] {
            let mut other = span.clone();
            other["attributes"]["custom"]["http"]["url"] = json!(observed);
            assert!(
                !matching_span(&other, &dd, request_id, revision, expected),
                "{observed}"
            );
        }
        for pointer in [
            "/attributes/service",
            "/attributes/env",
            "/attributes/custom/git/commit/sha",
            "/attributes/custom/dsf/request_id",
            "/attributes/status",
            "/attributes/custom/http/status_code",
        ] {
            let mut other = span.clone();
            *other.pointer_mut(pointer).unwrap() = json!("other");
            assert!(
                !matching_span(&other, &dd, request_id, revision, expected),
                "{pointer}"
            );
        }
        let mut other = span.clone();
        other["attributes"]["custom"]["http"]["url"] = json!(expected);
        assert!(!matching_span(
            &other,
            &dd,
            request_id,
            revision,
            "http://deep-sci-fi-staging.up.railway.app/api/health"
        ));
    }

    #[test]
    fn actual_nested_span_shape_must_match_service_environment_revision_and_probe() {
        let dd = Datadog {
            site: "datadoghq.com".into(),
            service: "backend".into(),
            environment: "production".into(),
            api_key_secret: "dd-api".into(),
            app_key_secret: "dd-app".into(),
        };
        let sha = "a".repeat(40);
        let span = json!({"attributes":{"service":"backend","env":"production","status":"ok",
            "custom":{"git":{"commit":{"sha":sha}},"dsf":{"request_id":"probe-1"},"http":{"status_code":"200","url":"https://api.deep-sci-fi.world/api/health"}}}});
        assert!(matching_span(
            &span,
            &dd,
            "probe-1",
            &sha,
            "https://api.deep-sci-fi.world/api/health"
        ));
        assert!(!matching_span(
            &span,
            &dd,
            "probe-1",
            &sha,
            "https://staging.deep-sci-fi.world/api/health"
        ));
        for pointer in [
            "/attributes/service",
            "/attributes/env",
            "/attributes/custom/git/commit/sha",
            "/attributes/custom/dsf/request_id",
        ] {
            let mut changed = span.clone();
            *changed.pointer_mut(pointer).unwrap() = "other".into();
            assert!(!matching_span(
                &changed,
                &dd,
                "probe-1",
                &sha,
                "https://api.deep-sci-fi.world/api/health"
            ));
        }
        let mut failed = span;
        failed["attributes"]["status"] = "error".into();
        assert!(!matching_span(
            &failed,
            &dd,
            "probe-1",
            &sha,
            "https://api.deep-sci-fi.world/api/health"
        ));
    }

    #[test]
    fn missing_or_unknown_success_status_does_not_prove_a_healthy_http_probe() {
        let dd = Datadog {
            site: "datadoghq.com".into(),
            service: "backend".into(),
            environment: "production".into(),
            api_key_secret: "dd-api".into(),
            app_key_secret: "dd-app".into(),
        };
        let sha = "a".repeat(40);
        let base = json!({"attributes":{"service":"backend","env":"production","status":"ok",
            "custom":{"git":{"commit":{"sha":sha}},"dsf":{"request_id":"probe-1"},"http":{"status_code":"200","url":"https://api.deep-sci-fi.world/api/health"}}}});
        for status in [Value::Null, json!("unknown"), json!("error")] {
            let mut span = base.clone();
            span["attributes"]["status"] = status;
            assert!(!matching_span(
                &span,
                &dd,
                "probe-1",
                &sha,
                "https://api.deep-sci-fi.world/api/health"
            ));
        }
        for status in [Value::Null, json!(500), json!("500")] {
            let mut span = base.clone();
            span["attributes"]["custom"]["http"]["status_code"] = status;
            assert!(!matching_span(
                &span,
                &dd,
                "probe-1",
                &sha,
                "https://api.deep-sci-fi.world/api/health"
            ));
        }
    }
}
