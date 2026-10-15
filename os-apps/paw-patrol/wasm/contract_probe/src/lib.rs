//! contract_probe — one concern: GET a pinned OData path and record latency.
//!
//! Fired by ContractProbe.RunScan. Reads path / filter / max_ms from the
//! entity. Does not take caller parameters. Does not dispatch.

use temper_wasm_sdk::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let path = field(&fields, "path").unwrap_or_else(|| "/tdata/DesignLanguages".into());
        let filter = field(&fields, "filter").unwrap_or_default();
        let max_ms = parse_i64(
            &field(&fields, "max_ms").unwrap_or_else(|| "800".into()),
            800,
        );
        let base = ctx
            .config
            .get("temper_api_url")
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
            .ok_or_else(|| "contract_probe: missing config temper_api_url".to_string())?;
        let key = ctx
            .config
            .get("temper_api_key")
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
            .unwrap_or_default();
        let url = probe_url(&base, &path, &filter)?;
        let headers = if key.is_empty() {
            vec![("accept".into(), "application/json".into())]
        } else {
            vec![
                ("accept".into(), "application/json".into()),
                ("authorization".into(), format!("Bearer {key}")),
                ("x-tenant-id".into(), "default".into()),
            ]
        };
        let t0 = Context::get_time_millis();
        let resp = ctx.http_call("GET", &url, &headers, "")?;
        let latency_ms = Context::get_time_millis().saturating_sub(t0);
        if resp.status >= 400 {
            return Err(format!(
                "contract_probe: HTTP {} in {latency_ms}ms",
                resp.status
            ));
        }
        let rows = odata_row_count(&resp.body)?;
        let passed = latency_ms <= max_ms;
        ctx.log(
            "info",
            &format!(
                "contract_probe: {path} rows={rows} {latency_ms}ms max={max_ms} passed={passed}"
            ),
        );
        set_success_result(
            "RunSucceeded",
            &json!({
                "latency_ms": latency_ms.to_string(),
                "row_count": rows.to_string(),
                "passed": if passed { "true" } else { "false" },
            }),
        );
        Ok(())
    })();
    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

fn field(fields: &Value, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let pascal = {
                let mut out = String::new();
                for part in key.split('_') {
                    let mut chars = part.chars();
                    if let Some(first) = chars.next() {
                        out.push_str(&first.to_uppercase().collect::<String>());
                        out.push_str(chars.as_str());
                    }
                }
                out
            };
            fields
                .get(&pascal)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        })
}

fn parse_i64(raw: &str, fallback: i64) -> i64 {
    raw.trim().parse().unwrap_or(fallback)
}

fn probe_url(base: &str, path: &str, filter: &str) -> Result<String, String> {
    if !path.starts_with("/tdata/") {
        return Err("contract_probe: path must start with /tdata/".into());
    }
    if path.contains(' ') || path.contains('\n') {
        return Err("contract_probe: path is not a single OData path".into());
    }
    let mut url = format!("{}{path}", base.trim_end_matches('/'));
    if !filter.is_empty() {
        if filter.contains('\n') || filter.contains('&') {
            return Err("contract_probe: filter is not a single $filter".into());
        }
        url.push_str("?$filter=");
        url.push_str(&odata_escape(filter));
    }
    Ok(url)
}

fn odata_escape(filter: &str) -> String {
    filter
        .replace(' ', "%20")
        .replace('\'', "%27")
        .replace('"', "%22")
}

fn odata_row_count(body: &str) -> Result<u64, String> {
    let v: Value = serde_json::from_str(body)
        .map_err(|_| "contract_probe: response is not JSON".to_string())?;
    v.get("value")
        .and_then(|x| x.as_array())
        .map(|a| a.len() as u64)
        .ok_or_else(|| "contract_probe: response is not an OData value array".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_requires_tdata() {
        assert!(probe_url("https://x", "/tdata/DesignLanguages", "").is_ok());
        assert!(probe_url("https://x", "/paw/version", "").is_err());
    }

    #[test]
    fn filter_is_single_clause() {
        let url = probe_url(
            "https://x",
            "/tdata/DesignLanguages",
            "id eq '__arn462_missing__'",
        )
        .unwrap();
        assert!(url.contains("%20eq%20"));
        assert!(probe_url("https://x", "/tdata/DesignLanguages", "a&b").is_err());
    }

    #[test]
    fn counts_odata_value() {
        assert_eq!(odata_row_count(r#"{"value":[]}"#).unwrap(), 0);
        assert_eq!(odata_row_count(r#"{"value":[{},{}]}"#).unwrap(), 2);
        assert!(odata_row_count("not-json").is_err());
        assert!(odata_row_count(r#"{"ok":true}"#).is_err());
    }
}
