//! Mint a GitHub App installation token the same way `chain_github_ready` does.
//!
//! App first (JWT → list installs → mint). Tenant `github_token` is the
//! fallback when the App is not configured or this owner has no install.

use temper_wasm_sdk::prelude::*;

#[cfg(target_arch = "wasm32")]
getrandom::register_custom_getrandom!(unsupported_random);

#[cfg(target_arch = "wasm32")]
fn unsupported_random(_buf: &mut [u8]) -> Result<(), getrandom::Error> {
    Err(getrandom::Error::UNSUPPORTED)
}

pub(crate) fn github_bearer(ctx: &Context, repo: &str) -> Result<String, String> {
    let owner = repo.split('/').next().unwrap_or("");
    debug_assert!(!owner.is_empty(), "caller validates owner/name before mint");
    let app_id = secret(ctx, "github_app_id");
    let pem = secret(ctx, "github_app_private_key");
    match (app_id, pem) {
        (Some(app_id), Some(pem)) => {
            if let Some(token) = installation_token(ctx, &app_id, &pem, owner)? {
                return Ok(token);
            }
            if let Some(token) = secret(ctx, "github_token") {
                return Ok(token);
            }
            return Err(format!(
                "release_run_lifecycle: GitHub App has no install on {owner} and github_token is empty"
            ));
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "release_run_lifecycle: GitHub App needs both github_app_id and github_app_private_key"
                    .to_string(),
            );
        }
        (None, None) => {}
    }
    secret(ctx, "github_token").ok_or_else(|| {
        "release_run_lifecycle: GitHub App (github_app_id + github_app_private_key) is not configured and github_token is empty"
            .to_string()
    })
}

fn secret(ctx: &Context, key: &str) -> Option<String> {
    ctx.config
        .get(key)
        .filter(|v| !v.is_empty() && !v.contains("{secret:"))
        .cloned()
}

fn installation_token(
    ctx: &Context,
    app_id: &str,
    pem: &str,
    owner: &str,
) -> Result<Option<String>, String> {
    let jwt = github_app_jwt(app_id, pem)?;
    let auth = vec![
        (
            "accept".to_string(),
            "application/vnd.github+json".to_string(),
        ),
        (
            "user-agent".to_string(),
            "temperpaw-release-run-lifecycle".to_string(),
        ),
        ("authorization".to_string(), format!("Bearer {jwt}")),
    ];
    let resp = ctx.http_call(
        "GET",
        "https://api.github.com/app/installations?per_page=100",
        &auth,
        "",
    )?;
    if resp.status >= 400 {
        return Err(format!(
            "release_run_lifecycle: list installations HTTP {}",
            resp.status
        ));
    }
    let installs: Value = serde_json::from_str(&resp.body)
        .map_err(|e| format!("release_run_lifecycle: installations: {e}"))?;
    let Some(id) = install_id_for_owner(&installs, owner) else {
        return Ok(None);
    };
    let url = format!("https://api.github.com/app/installations/{id}/access_tokens");
    let minted = ctx.http_call("POST", &url, &auth, "")?;
    if minted.status >= 400 {
        return Err(format!(
            "release_run_lifecycle: mint installation token HTTP {}",
            minted.status
        ));
    }
    let body: Value = serde_json::from_str(&minted.body)
        .map_err(|e| format!("release_run_lifecycle: access_token: {e}"))?;
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "release_run_lifecycle: installation token missing".to_string())?;
    Ok(Some(token.to_string()))
}

pub(crate) fn install_id_for_owner(installs: &Value, owner: &str) -> Option<u64> {
    let rows = installs.as_array()?;
    for row in rows {
        let login = row
            .get("account")
            .and_then(|a| a.get("login"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if login.eq_ignore_ascii_case(owner) {
            return row.get("id").and_then(|v| v.as_u64());
        }
    }
    None
}

pub(crate) fn github_app_jwt(app_id: &str, pem: &str) -> Result<String, String> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs1v15::Pkcs1v15Sign;
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::sha2::{Digest, Sha256};
    use rsa::RsaPrivateKey;

    let now = now_secs()?;
    let header = b64url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = b64url(
        format!(
            r#"{{"iat":{},"exp":{},"iss":"{app_id}"}}"#,
            now.saturating_sub(60),
            now + 540
        )
        .as_bytes(),
    );
    let signing_input = format!("{header}.{payload}");
    let key = RsaPrivateKey::from_pkcs1_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem))
        .map_err(|e| format!("release_run_lifecycle: github_app_private_key: {e}"))?;
    let digest = Sha256::digest(signing_input.as_bytes());
    let sig = key
        .sign(Pkcs1v15Sign::new::<Sha256>(), &digest)
        .map_err(|e| format!("release_run_lifecycle: jwt sign: {e}"))?;
    Ok(format!("{signing_input}.{}", b64url(&sig)))
}

pub(crate) fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
        out.push(T[(b0 >> 2) as usize] as char);
        out.push(T[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(T[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(T[(b2 & 0x3f) as usize] as char);
        }
        i += 3;
    }
    out
}

fn now_secs() -> Result<u64, String> {
    Ok((Context::get_time_millis() / 1000) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64url_has_no_padding() {
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
    }

    #[test]
    fn picks_install_for_owner() {
        let installs = json!([
            {"id": 11, "account": {"login": "nerdsane"}},
            {"id": 22, "account": {"login": "arni-labs"}}
        ]);
        assert_eq!(install_id_for_owner(&installs, "arni-labs"), Some(22));
        assert_eq!(install_id_for_owner(&installs, "ARNI-LABS"), Some(22));
        assert_eq!(install_id_for_owner(&installs, "missing"), None);
    }
}
