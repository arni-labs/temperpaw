use temper_wasm_sdk::prelude::*;
pub fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    let pascal: String = name
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    row.get(name)
        .or_else(|| row.get(&pascal))
        .or_else(|| row.get("fields").and_then(|f| f.get(name)))
        .and_then(Value::as_str)
        .unwrap_or("")
}
pub fn get_json(ctx: &Context, path: &str) -> Result<Value, String> {
    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("temper_api_url secret is not configured")?;
    let headers = vec![
        ("x-tenant-id".into(), ctx.tenant.clone()),
        ("x-temper-principal-kind".into(), "agent".into()),
        ("x-temper-principal-id".into(), ctx.entity_id.clone()),
        ("x-temper-agent-type".into(), "system".into()),
    ];
    let response = ctx.http_call("GET", &format!("{api}/tdata/{path}"), &headers, "")?;
    if !(200..300).contains(&response.status) {
        return Err(format!("GET {path}: HTTP {}", response.status));
    }
    if response.body.len() > 2_000_000 {
        return Err("entity response exceeds 2 MB bound".into());
    }
    serde_json::from_str(&response.body).map_err(|error| error.to_string())
}
pub fn safe_id(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        Err("invalid entity identity".into())
    } else {
        Ok(value)
    }
}
pub fn model(raw: &str, mode: &str) -> Result<foresight_learning_core::Model, String> {
    let parsed = if raw.is_empty() {
        foresight_learning_core::Model::identity(mode)
    } else {
        serde_json::from_str(raw).map_err(|e| format!("invalid model: {e}"))?
    };
    parsed.validate()?;
    Ok(parsed)
}
