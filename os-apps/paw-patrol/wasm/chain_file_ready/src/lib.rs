//! chain_file_ready — one concern: the named Temper File exists and is Ready.
//!
//! Fired by AttachReviewFile / AttachProofFile. Reads the File id from
//! the trigger param or entity field named in config `file_id_field`, GETs
//! `/tdata/Files('<id>')`, and requires status Ready or Locked (paw-fs:
//! those states always have content). On any miss, set_error_result so the
//! spec's on_failure retracts the ready bool.
//!
//! Does not dispatch. Does not write files.

use temper_wasm_sdk::prelude::*;

const READY_STATUSES: &[&str] = &["Ready", "Locked"];

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let field = ctx
            .config
            .get("file_id_field")
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "chain_file_ready: missing config file_id_field".to_string())?;
        let file_id = file_id_from(&ctx, &fields, field)?;
        let base_url = resolve_api_url(&ctx);
        let headers = odata_headers(&ctx);
        let body = get_file(&ctx, &base_url, &headers, &file_id)?;
        let status = file_status(&body).unwrap_or_default();
        if !file_is_ready(&status) {
            return Err(format!(
                "chain_file_ready: File {file_id} status {status:?} is not Ready or Locked"
            ));
        }
        ctx.log(
            "info",
            &format!("chain_file_ready: File {file_id} is {status}"),
        );
        set_success_result("", &json!({ "file_id": file_id, "status": status }));
        Ok(())
    })();
    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

fn file_id_from(ctx: &Context, fields: &Value, field: &str) -> Result<String, String> {
    let from_param = ctx
        .trigger_params
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let from_field = fields
        .get(field)
        .and_then(|v| v.as_str())
        .or_else(|| fields.get(&pascal(field)).and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    let id = if !from_param.is_empty() {
        from_param
    } else {
        from_field
    };
    if id.is_empty() {
        return Err(format!("chain_file_ready: empty {field}"));
    }
    if id.contains('\'') || id.contains('/') {
        return Err(format!("chain_file_ready: {field} is not a File id"));
    }
    Ok(id.to_string())
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

fn file_is_ready(status: &str) -> bool {
    READY_STATUSES.contains(&status)
}

fn file_status(body: &Value) -> Option<String> {
    let fields = body.get("fields").unwrap_or(body);
    fields
        .get("status")
        .and_then(|v| v.as_str())
        .or_else(|| fields.get("Status").and_then(|v| v.as_str()))
        .map(str::to_string)
}

fn resolve_api_url(ctx: &Context) -> String {
    ctx.config
        .get("temper_api_url")
        .filter(|value| !value.is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string())
}

fn odata_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "agent".to_string()),
        ("x-temper-principal-id".to_string(), ctx.entity_id.clone()),
        ("x-temper-agent-type".to_string(), "system".to_string()),
    ]
}

fn get_file(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    file_id: &str,
) -> Result<Value, String> {
    let url = format!(
        "{}/tdata/Files('{}')",
        base_url.trim_end_matches('/'),
        file_id
    );
    let resp = ctx.http_call("GET", &url, headers, "")?;
    if resp.status >= 400 {
        return Err(format!(
            "chain_file_ready: GET File {file_id} HTTP {}",
            resp.status
        ));
    }
    serde_json::from_str(&resp.body)
        .map_err(|e| format!("chain_file_ready: File {file_id} body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_and_locked_pass() {
        assert!(file_is_ready("Ready"));
        assert!(file_is_ready("Locked"));
        assert!(!file_is_ready("Created"));
        assert!(!file_is_ready(""));
    }

    #[test]
    fn status_from_odata_wrapper_or_flat() {
        let wrapped = json!({"fields": {"status": "Ready"}});
        assert_eq!(file_status(&wrapped).as_deref(), Some("Ready"));
        let flat = json!({"Status": "Locked"});
        assert_eq!(file_status(&flat).as_deref(), Some("Locked"));
    }

    #[test]
    fn pascal_spec_ref() {
        assert_eq!(pascal("spec_ref"), "SpecRef");
        assert_eq!(pascal("intent_ref"), "IntentRef");
    }
}
