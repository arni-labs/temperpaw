//! Immutable, lossless saved assessment context. No references to mutable run data.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub const MAX_CONTEXT_BYTES: usize = 8 * 1024 * 1024;
fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
pub fn encode(snapshot: &str, program: &str, trace: &str) -> Result<Value, String> {
    let raw =
        json!({"snapshot_json":snapshot,"program_json":program,"trace_json":trace}).to_string();
    if raw.len() > MAX_CONTEXT_BYTES {
        return Err("checkpoint_context_bound".into());
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(raw.as_bytes(), 6);
    Ok(
        json!({"version":2,"context_encoding":"zlib-base64","context_data":STANDARD.encode(compressed),"context_uncompressed_bytes":raw.len(),"snapshot_sha256":digest(snapshot),"program_sha256":digest(program),"trace_sha256":digest(trace)}),
    )
}
#[cfg(test)]
pub fn decode(checkpoint: &Value) -> Result<Value, String> {
    let len = checkpoint["context_uncompressed_bytes"]
        .as_u64()
        .ok_or("Missing decoded size")? as usize;
    if len > MAX_CONTEXT_BYTES {
        return Err("Decoded size exceeds bound".into());
    }
    let bytes = STANDARD
        .decode(
            checkpoint["context_data"]
                .as_str()
                .ok_or("Missing context")?,
        )
        .map_err(|e| e.to_string())?;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&bytes, len)
        .map_err(|e| format!("Invalid compressed context: {e:?}"))?;
    if raw.len() != len {
        return Err("Decoded size differs".into());
    }
    let value: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    for key in ["snapshot", "program", "trace"] {
        let raw = value[format!("{key}_json")]
            .as_str()
            .ok_or("Missing original context")?;
        if checkpoint[format!("{key}_sha256")] != digest(raw) {
            return Err("Original context hash differs".into());
        }
    }
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_context_is_immutable_and_corruption_rejected() {
        let original = "{\"number\":1.0,\"escaped\":\"a\\nb\"}";
        let saved = encode(original, "{\"stage\":\"old\"}", "[]").unwrap();
        let decoded = decode(&saved).unwrap();
        assert_eq!(decoded["snapshot_json"], original);
        let mut current = json!({"stage":"old"});
        current["stage"] = json!("new");
        assert_ne!(decoded["program_json"], current.to_string());
        let mut corrupt = saved.clone();
        corrupt["program_sha256"] = json!("0".repeat(64));
        assert!(decode(&corrupt).is_err());
        corrupt = saved.clone();
        corrupt["context_data"] = json!("AAAA");
        assert!(decode(&corrupt).is_err());
        corrupt = saved;
        corrupt["context_uncompressed_bytes"] = json!(MAX_CONTEXT_BYTES + 1);
        assert!(decode(&corrupt).is_err());
    }
}

#[cfg(test)]
mod captured_tests {
    use super::*;
    #[test]
    #[ignore = "requires frozen authorized pass19 capture"]
    fn captured_context_round_trip_and_storage_bound() {
        let capture: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("FORESIGHT_COMPRESSED_CAPTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let f = &capture["fields"];
        let mut saved = encode(
            f["snapshot_json"].as_str().unwrap(),
            f["program_json"].as_str().unwrap(),
            f["trace_json"].as_str().unwrap(),
        )
        .unwrap();
        let decoded = decode(&saved).unwrap();
        for key in ["snapshot_json", "program_json", "trace_json"] {
            assert_eq!(decoded[key], f[key]);
        }
        saved["answer"] = serde_json::from_str::<Value>(f["answer"].as_str().unwrap()).unwrap();
        let mut p: Value = serde_json::from_str(f["program_json"].as_str().unwrap()).unwrap();
        p["answer_checkpoint"] = saved;
        assert!(p.to_string().len() < MAX_CONTEXT_BYTES);
    }
}
