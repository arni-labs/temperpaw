//! Provider reads are bound by Rust types, never a runtime provider selector.
use super::*;
pub struct Railway;
pub struct Vercel;
pub struct Supabase;
pub struct R2;
pub struct Datadog;
pub struct Media;
const RAILWAY_QUERY: &str = "query DsfRailwayObservation($serviceId:String!,$environmentId:String!){service(id:$serviceId){id projectId} serviceInstance(serviceId:$serviceId,environmentId:$environmentId){id serviceId environmentId startCommand buildCommand rootDirectory healthcheckPath healthcheckTimeout region numReplicas restartPolicyType restartPolicyMaxRetries sleepApplication latestDeployment{id status createdAt meta} activeDeployments{id status createdAt meta}}}";
impl Collector for Railway {
    type Binding = dsf_railway_actions::Deploy;
    const NOT_FOUND_IS_ABSENT: bool = false;
    fn source(_: &dsf_railway_actions::Target) -> String {
        "https://backboard.railway.com/graphql/v2".into()
    }
    fn query(t: &dsf_railway_actions::Target) -> String {
        json!({"query":RAILWAY_QUERY,"variables":{"serviceId":t.service_id,"environmentId":t.environment_id}}).to_string()
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_railway_actions::Target) -> Result<Facts, Error> {
        let row = r.bearer_json(
            &t.token_secret,
            "POST",
            Self::source(t),
            serde_json::from_str(&Self::query(t)).expect("constructed query"),
        )?;
        let service = row
            .pointer("/data/service")
            .ok_or(Error::Response("Railway service"))?;
        if service.is_null() {
            return Ok(Facts::unavailable(
                Coverage::Absent,
                "service_not_found",
                json!({"project_id":t.project_id,"service_id":t.service_id,"service":null}),
            ));
        }
        if required(service, "id")? != t.service_id
            || required(service, "projectId")? != t.project_id
        {
            return Err(Error::Binding("Railway service identity differs"));
        }
        let instance = row
            .pointer("/data/serviceInstance")
            .ok_or(Error::Response("Railway service instance"))?;
        if instance.is_null() {
            return Ok(Facts::unavailable(
                Coverage::Absent,
                "service_instance_not_found",
                json!({"service_id":t.service_id,"environment_id":t.environment_id,"instance":null}),
            ));
        }
        if required(instance, "serviceId")? != t.service_id
            || required(instance, "environmentId")? != t.environment_id
        {
            return Err(Error::Binding("Railway environment differs"));
        }
        let mut values = picked(
            instance,
            &[
                "id",
                "serviceId",
                "environmentId",
                "startCommand",
                "buildCommand",
                "rootDirectory",
                "healthcheckPath",
                "healthcheckTimeout",
                "region",
                "numReplicas",
                "restartPolicyType",
                "restartPolicyMaxRetries",
                "sleepApplication",
            ],
        )?;
        values["project_id"] = json!(t.project_id);
        let latest = instance
            .get("latestDeployment")
            .ok_or(Error::Response("Railway deployment field"))?;
        let describe = |deployment: &Value| -> Result<Value, Error> {
            required(deployment, "id")?;
            required(deployment, "status")?;
            let mut facts = picked(deployment, &["id", "status", "createdAt"])?;
            facts["revision"] = json!(revision(deployment.pointer("/meta/commitHash"))?);
            Ok(facts)
        };
        values["latest_deployment"] = if latest.is_null() {
            Value::Null
        } else {
            describe(latest)?
        };
        let active = instance
            .get("activeDeployments")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() <= 10)
            .ok_or(Error::Response("Railway active deployments"))?;
        let active = active.iter().map(describe).collect::<Result<Vec<_>, _>>()?;
        let (state, sha) = if active.len() == 1 {
            let state = required(&active[0], "status")?;
            (
                state.to_owned(),
                if state == "SUCCESS" {
                    active[0]["revision"].as_str().unwrap_or("").to_owned()
                } else {
                    String::new()
                },
            )
        } else {
            (
                if active.is_empty() {
                    "no_active_deployment"
                } else {
                    "multiple_active_deployments"
                }
                .into(),
                String::new(),
            )
        };
        values["active_deployments"] = json!(active);
        Ok(Facts::measured(&state, &sha, values))
    }
}
impl Collector for Vercel {
    type Binding = dsf_vercel_actions::Deploy;
    fn source(t: &dsf_vercel_actions::Target) -> String {
        format!(
            "https://api.vercel.com/v9/projects/{}?teamId={}",
            encoded(&t.project_id),
            encoded(&t.account_id)
        )
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_vercel_actions::Target) -> Result<Facts, Error> {
        let row = r.bearer_json(&t.token_secret, "GET", Self::source(t), Value::Null)?;
        if required(&row, "id")? != t.project_id
            || required(&row, "accountId")? != t.account_id
            || required(&row, "name")? != t.project_name
        {
            return Err(Error::Binding("Vercel project or account differs"));
        }
        let mut values = picked(
            &row,
            &[
                "id",
                "accountId",
                "buildCommand",
                "installCommand",
                "outputDirectory",
                "rootDirectory",
                "framework",
                "nodeVersion",
            ],
        )?;
        let deployment = row.pointer("/targets/production");
        if deployment.is_none_or(Value::is_null) {
            values["production_deployment"] = Value::Null;
            return Ok(Facts::measured("no_production_deployment", "", values));
        }
        let deployment = deployment.expect("checked optional deployment");
        let sha = revision(
            deployment
                .pointer("/gitSource/sha")
                .or_else(|| deployment.pointer("/meta/githubCommitSha")),
        )?;
        if let Some(other) = deployment
            .pointer("/meta/githubCommitSha")
            .and_then(Value::as_str)
            && other != sha
        {
            return Err(Error::Response("conflicting Vercel revisions"));
        }
        let state = required(deployment, "readyState")?;
        required(deployment, "id")?;
        values["production_deployment"] =
            picked(deployment, &["id", "readyState", "url", "createdAt"])?;
        values["production_deployment"]["revision"] = json!(sha);
        Ok(Facts::measured(state, &sha, values))
    }
}
impl Collector for Supabase {
    type Binding = dsf_supabase_actions::ApplyConfiguration;
    const NOT_FOUND_IS_ABSENT: bool = false;
    fn source(t: &dsf_supabase_actions::Target) -> String {
        format!(
            "https://api.supabase.com/v1/projects/{}",
            encoded(&t.project_ref)
        )
    }
    fn query(t: &dsf_supabase_actions::Target) -> String {
        json!({"reads":[Self::source(t),format!("{}/config/database/postgres",Self::source(t))]})
            .to_string()
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_supabase_actions::Target) -> Result<Facts, Error> {
        let row = r.bearer_json(&t.token_secret, "GET", Self::source(t), Value::Null)?;
        if required(&row, "ref")? != t.project_ref {
            return Err(Error::Binding("Supabase project differs"));
        }
        let state = required(&row, "status")?;
        let mut values = picked(&row, &["id", "ref", "region", "status", "created_at"])?;
        let config = r.bearer_json(
            &t.token_secret,
            "GET",
            format!("{}/config/database/postgres", Self::source(t)),
            Value::Null,
        )?;
        if !config.is_object() {
            return Err(Error::Response("Supabase postgres configuration"));
        }
        values["postgres"] = picked(
            &config,
            &[
                "statement_timeout",
                "work_mem",
                "max_connections",
                "log_connections",
                "log_disconnections",
                "log_lock_waits",
            ],
        )?;
        Ok(Facts::measured(state, "", values))
    }
}
impl Collector for R2 {
    type Binding = dsf_r2_actions::ApplyConfiguration;
    fn source(t: &dsf_r2_actions::Target) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}",
            encoded(&t.account_id),
            encoded(&t.bucket_name)
        )
    }
    fn query(t: &dsf_r2_actions::Target) -> String {
        json!({"reads":[Self::source(t),format!("{}/cors",Self::source(t))]}).to_string()
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_r2_actions::Target) -> Result<Facts, Error> {
        let row = r.bearer_json(&t.token_secret, "GET", Self::source(t), Value::Null)?;
        if row.get("success") != Some(&json!(true)) {
            return Err(Error::Response("Cloudflare bucket envelope"));
        }
        let bucket = row
            .get("result")
            .ok_or(Error::Response("Cloudflare bucket"))?;
        if required(bucket, "name")? != t.bucket_name {
            return Err(Error::Binding("Cloudflare bucket differs"));
        }
        let mut values = picked(
            bucket,
            &[
                "name",
                "creation_date",
                "jurisdiction",
                "location",
                "storage_class",
            ],
        )?;
        let cors = r
            .bearer_json(
                &t.token_secret,
                "GET",
                format!("{}/cors", Self::source(t)),
                Value::Null,
            )
            .map_err(|_| Error::Response("Cloudflare CORS unavailable"))?;
        if cors.get("success") != Some(&json!(true)) {
            return Err(Error::Response("Cloudflare CORS envelope"));
        }
        let cors: dsf_r2_actions::Change = serde_json::from_value(
            cors.get("result")
                .cloned()
                .ok_or(Error::Response("Cloudflare CORS"))?,
        )
        .map_err(|_| Error::Response("Cloudflare CORS"))?;
        if cors.rules.len() > 100 {
            return Err(Error::Response("Cloudflare CORS rules bound"));
        }
        values["cors"] =
            serde_json::to_value(cors).map_err(|_| Error::Response("Cloudflare CORS"))?;
        Ok(Facts::measured("bucket_present", "", values))
    }
}
impl Collector for Datadog {
    type Binding = dsf_datadog_actions::ApplyConfiguration;
    fn source(t: &dsf_datadog_actions::Target) -> String {
        format!("https://api.{}/api/v1/monitor/{}", t.site, t.monitor_id)
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_datadog_actions::Target) -> Result<Facts, Error> {
        datadog_site(&t.site)?;
        let api = r.credential(&t.api_key_secret)?;
        let app = r.credential(&t.app_key_secret)?;
        let row = json_body(
            r.host.request(&Request {
                method: "GET",
                url: Self::source(t),
                headers: vec![
                    ("DD-API-KEY".into(), api),
                    ("DD-APPLICATION-KEY".into(), app),
                ],
                body: String::new(),
            })?,
            "Datadog monitor",
        )?;
        if row.get("id").and_then(Value::as_u64) != Some(t.monitor_id) {
            return Err(Error::Binding("Datadog monitor differs"));
        }
        let state = required(&row, "overall_state")?;
        let mut values = picked(
            &row,
            &[
                "id",
                "type",
                "query",
                "overall_state",
                "overall_state_modified",
                "created",
                "modified",
                "priority",
            ],
        )?;
        if let Some(options) = row.get("options") {
            values["options"] = picked(
                options,
                &[
                    "notify_no_data",
                    "no_data_timeframe",
                    "require_full_window",
                    "include_tags",
                    "evaluation_delay",
                    "new_group_delay",
                ],
            )?;
            if let Some(thresholds) = options.get("thresholds") {
                values["options"]["thresholds"] = picked(
                    thresholds,
                    &[
                        "critical",
                        "warning",
                        "critical_recovery",
                        "warning_recovery",
                    ],
                )?;
            }
        }
        // Monitor state is a provider configuration read, not a metric sample.
        Ok(Facts::measured(state, "", values))
    }
}
impl Collector for Media {
    type Binding = dsf_media_actions::RetrySelected;
    const NOT_FOUND_IS_ABSENT: bool = false;
    fn source(_: &dsf_media_actions::Target) -> String {
        format!("{DSF_API}/api/operations/snapshot?participant_limit=1&job_limit=20")
    }
    fn read(r: &mut Runtime<impl Host>, t: &dsf_media_actions::Target) -> Result<Facts, Error> {
        let row = r.bearer_json(&t.token_secret, "GET", Self::source(t), Value::Null)?;
        if row.get("snapshot_version") != Some(&json!(1))
            || required(&row, "service")? != "deep-sci-fi-backend"
            || required(&row, "environment")? != "production"
            || row.get("participant_limit") != Some(&json!(1))
            || row.get("job_limit") != Some(&json!(20))
        {
            return Err(Error::Binding("DSF snapshot identity or bounds differ"));
        }
        let at = DateTime::parse_from_rfc3339(required(&row, "observed_at")?)
            .map_err(|_| Error::Response("DSF snapshot time"))?
            .timestamp_millis();
        if at > r.now_ms + 5000 {
            return Err(Error::Response("future DSF snapshot"));
        }
        let media = row.get("media").ok_or(Error::Response("DSF media queue"))?;
        let counts = media
            .get("counts")
            .and_then(Value::as_object)
            .filter(|m| m.len() <= 32 && m.iter().all(|(key, v)| key.len() <= 64 && v.is_u64()))
            .ok_or(Error::Response("DSF media counts"))?;
        let jobs = media
            .get("jobs")
            .and_then(Value::as_array)
            .filter(|a| a.len() <= 20)
            .ok_or(Error::Response("DSF media jobs bound"))?;
        let has_more = media
            .get("has_more")
            .and_then(Value::as_bool)
            .ok_or(Error::Response("DSF media coverage"))?;
        let mut selected = Vec::new();
        for job in jobs {
            for key in ["id", "participant_id", "status", "created_at"] {
                required(job, key)?;
            }
            for key in ["age_seconds", "attempts"] {
                if !job.get(key).is_some_and(Value::is_u64) {
                    return Err(Error::Response("DSF media job counter"));
                }
            }
            if !job.get("retry_eligible").is_some_and(Value::is_boolean) {
                return Err(Error::Response("DSF media eligibility"));
            }
            selected.push(picked(
                job,
                &[
                    "id",
                    "participant_id",
                    "status",
                    "created_at",
                    "age_seconds",
                    "attempts",
                    "retry_eligible",
                ],
            )?);
        }
        let sha = revision(row.get("revision"))?;
        let mut result = Facts::measured(
            "media_queue_observed",
            &sha,
            json!({"service":"deep-sci-fi-backend","environment":"production","snapshot_version":1,"observed_at":row["observed_at"],"revision":sha,"job_limit":20,"counts":counts,"jobs":selected,"has_more":has_more,"oldest_unfinished_at":media.get("oldest_unfinished_at")}),
        );
        result.sample_kind = "operational_snapshot";
        result.source_at_ms = Some(at);
        if r.now_ms.saturating_sub(at) > 60000 {
            result.coverage = Coverage::Stale;
            result.outcome = "stale_media_snapshot".into();
        }
        Ok(result)
    }
}
