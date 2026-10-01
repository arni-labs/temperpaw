pub fn checked_packet(raw: &str, expected_hash: &str) -> Result<serde_json::Value, String> {
    use sha2::{Digest, Sha256};
    if raw.len() > 128 * 1024 {
        return Err("evaluation_packet_exceeds_128k_bytes".into());
    }
    if format!("{:x}", Sha256::digest(raw.as_bytes())) != expected_hash {
        return Err("evaluation_packet_hash_mismatch".into());
    }
    let p: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "invalid_evaluation_packet")?;
    if p["schema_version"] != "foresight-evaluation-packet-v2" {
        return Err("evaluation_packet_version_mismatch".into());
    }
    match p["mode"].as_str() {
        Some("off") => {}
        Some("shadow") => {
            let expected: serde_json::Value =
                serde_json::from_str(include_str!("evaluation_contract.json"))
                    .map_err(|_| "invalid_contract")?;
            if p["questions"] != expected {
                return Err("evaluation_contract_mismatch".into());
            }
            for key in ["repair", "endpoint_bundle", "observed_graph"] {
                if p["state"][key].as_str().is_none_or(|s| s.trim().is_empty()) {
                    return Err("incomplete_evaluation_packet".into());
                }
            }
        }
        _ => return Err("invalid_semantic_evaluation_mode".into()),
    }
    Ok(p)
}
