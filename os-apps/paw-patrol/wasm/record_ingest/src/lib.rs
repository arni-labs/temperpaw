//! record_ingest - stage 3 S0 shadow bridge (ARN-430). TEMPORARY, retired at S3.
//!
//! Fired by ReviewRun.Ingest / ProofPacket.Ingest on a raw GitHub comment body.
//! It parses the comment's base64 record marker and RETURNS a callback: the
//! write action (`IngestRecord` for a review record, `IngestProof` for a proof
//! record) plus the decoded fields as its params. The kernel applies that
//! callback - the state machine writes the fields, not the module. The module
//! creates no entities and makes no OData calls of its own.
//!
//! No-op: when there is nothing valid to write for this entity, the module
//! returns an EMPTY callback action. The kernel builds `callback_action` from
//! the result's `action` (defaulting to "") and only dispatches when it is
//! non-empty (temper-server dispatch/wasm.rs at rev 43f9379), so an empty action
//! dispatches nothing. (Returning the SDK default "callback" would NOT be inert
//! here: that zeroing only applies to Composite integrations, so a plain trigger
//! returning "callback" would try to dispatch a non-existent `callback` action.)
//!
//! parse_ok is strict, typed extraction - every field the write actions map is
//! required with the right JSON type, else parse_ok is false and `reason` names
//! the offending field. On top of the types, proofs must satisfy the stack proof
//! rules (proof/validate.py): the changed + blast_radius surface present in
//! `features[]` with `verification == "rerun"`, `independent_verifier` agreeing
//! and covering that surface, no `verdict == "fail"`, UI evidence, tests passing.

use std::collections::BTreeMap;

use temper_wasm_sdk::prelude::*;

const REVIEW_MARKER: &str = "sdlc-review-record-b64";
const PROOF_MARKER: &str = "sdlc-proof-record-b64";
const CLOSE: &str = "-->";

/// Entity automaton names this module is fired on.
const REVIEW_ENTITY: &str = "ReviewRun";
const PROOF_ENTITY: &str = "ProofPacket";

/// Write actions the module asks the kernel to dispatch.
const INGEST_REVIEW_ACTION: &str = "IngestRecord";
const INGEST_PROOF_ACTION: &str = "IngestProof";
/// The no-op signal: an empty callback action the kernel does not dispatch.
const NO_DISPATCH: &str = "";

/// The decoded result of one comment body.
pub struct ParsedRecord {
    /// "review", "proof", or "none".
    pub kind: &'static str,
    /// The decoded record as JSON (an empty object when none/malformed).
    pub record: Value,
    /// Whether a well-formed, acceptable record was decoded.
    pub parse_ok: bool,
    /// The record's commit sha (empty when none/malformed).
    pub commit: String,
    /// Why parsing failed, naming the offending field (empty when parse_ok).
    pub reason: String,
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let ctx = match Context::from_host() {
        Ok(c) => c,
        Err(e) => {
            set_error_result(&e.to_string());
            return 0;
        }
    };
    let body = ctx
        .trigger_params
        .get("comment_body")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let parsed = parse_record(body);
    let action = ingest_action(&ctx.entity_type, &parsed);
    if action == NO_DISPATCH {
        // Empty action => the kernel dispatches nothing. Success, no error: a
        // comment with no valid record for this entity is a normal outcome.
        set_success_result(NO_DISPATCH, &json!({}));
    } else {
        set_success_result(action, &write_params(&parsed));
    }
    0
}

/// The write action the kernel should dispatch for this entity + record, or
/// `NO_DISPATCH` (empty) when there is nothing valid to write here.
pub fn ingest_action(entity_type: &str, parsed: &ParsedRecord) -> &'static str {
    if !parsed.parse_ok {
        return NO_DISPATCH;
    }
    match (entity_type, parsed.kind) {
        (REVIEW_ENTITY, "review") => INGEST_REVIEW_ACTION,
        (PROOF_ENTITY, "proof") => INGEST_PROOF_ACTION,
        _ => NO_DISPATCH,
    }
}

/// The decoded record flattened into the write action's params. Arrays and
/// objects are serialized to JSON strings to match the entities' string fields;
/// `open_act_on_count` is derived (unresolved act-on findings).
pub fn write_params(parsed: &ParsedRecord) -> Value {
    let r = &parsed.record;
    match parsed.kind {
        "review" => json!({
            "commit": parsed.commit,
            "reviewers_ran": json_string(r.get("reviewers_ran")),
            "findings": json_string(r.get("findings")),
            "risk": r.get("risk").and_then(|v| v.as_str()).unwrap_or(""),
            "open_act_on_count": open_act_on_count(r).to_string(),
        }),
        "proof" => json!({
            "commit": parsed.commit,
            "changed_surface": json_string(r.get("changed_surface")),
            "blast_radius": json_string(r.get("blast_radius")),
            "features": json_string(r.get("features")),
            "tests": json_string(r.get("tests")),
            "independent_verifier": json_string(r.get("independent_verifier")),
        }),
        _ => json!({}),
    }
}

/// Number of unresolved act-on findings (`severity == "act-on"` and not
/// `resolved`). The record is already type-checked when this runs.
pub fn open_act_on_count(record: &Value) -> usize {
    record
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|findings| {
            findings
                .iter()
                .filter(|f| {
                    f.get("severity").and_then(|s| s.as_str()) == Some("act-on")
                        && f.get("resolved").and_then(|r| r.as_bool()) != Some(true)
                })
                .count()
        })
        .unwrap_or(0)
}

fn json_string(v: Option<&Value>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

/// Parse a comment body into its record fields. Review marker wins over proof
/// when both are present (they never are in one real comment).
pub fn parse_record(body: &str) -> ParsedRecord {
    if let Some(payload) = extract_payload(body, REVIEW_MARKER) {
        return decode("review", &payload);
    }
    if let Some(payload) = extract_payload(body, PROOF_MARKER) {
        return decode("proof", &payload);
    }
    ParsedRecord {
        kind: "none",
        record: json!({}),
        parse_ok: false,
        commit: String::new(),
        reason: "no record marker found".to_string(),
    }
}

/// The base64 payload between a `<marker>` tag and the next `-->`, whitespace
/// stripped. `None` when the marker is absent or unterminated.
fn extract_payload(body: &str, marker: &str) -> Option<String> {
    let after_marker = body.find(marker)? + marker.len();
    let rest = &body[after_marker..];
    let end = rest.find(CLOSE)?;
    Some(rest[..end].chars().filter(|c| !c.is_whitespace()).collect())
}

fn decode(kind: &'static str, b64: &str) -> ParsedRecord {
    let bytes = match base64_decode(b64) {
        Some(b) => b,
        None => return failed(kind, "payload is not valid base64"),
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return failed(kind, "payload is not valid UTF-8"),
    };
    let record: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return failed(kind, "payload is not valid JSON"),
    };
    if !record.is_object() {
        return failed(kind, "record is not a JSON object");
    }
    match validate(kind, &record) {
        Ok(()) => {
            let commit = record
                .get("commit")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            ParsedRecord {
                kind,
                record,
                parse_ok: true,
                commit,
                reason: String::new(),
            }
        }
        Err(reason) => ParsedRecord {
            kind,
            record: json!({}),
            parse_ok: false,
            commit: String::new(),
            reason,
        },
    }
}

fn failed(kind: &'static str, reason: &str) -> ParsedRecord {
    ParsedRecord {
        kind,
        record: json!({}),
        parse_ok: false,
        commit: String::new(),
        reason: reason.to_string(),
    }
}

fn validate(kind: &str, record: &Value) -> Result<(), String> {
    match kind {
        "review" => validate_review(record),
        "proof" => validate_proof(record),
        _ => Err("unknown record kind".to_string()),
    }
}

/// Strict typed extraction of a review record: every mapped field present with
/// the right JSON type, else `Err` naming the field.
fn validate_review(r: &Value) -> Result<(), String> {
    let commit = str_field(r, "commit")?;
    if !is_full_sha(commit) {
        return Err("commit is not a 40-char lowercase-hex sha".to_string());
    }
    if str_array_field(r, "reviewers_ran")?.is_empty() {
        return Err("reviewers_ran is empty".to_string());
    }
    for (i, f) in array_field(r, "findings")?.iter().enumerate() {
        object(f).map_err(|e| format!("findings[{i}] {e}"))?;
        let severity = str_field(f, "severity").map_err(|e| format!("findings[{i}].{e}"))?;
        // Enum-checked so open_act_on_count can trust `severity == "act-on"`.
        if !matches!(severity, "act-on" | "consider" | "nit") {
            return Err(format!(
                "findings[{i}].severity '{severity}' is not act-on/consider/nit"
            ));
        }
        str_field(f, "file_line").map_err(|e| format!("findings[{i}].{e}"))?;
        bool_field(f, "resolved").map_err(|e| format!("findings[{i}].{e}"))?;
    }
    let risk = str_field(r, "risk")?;
    if !matches!(risk, "low" | "medium" | "high") {
        return Err(format!("risk '{risk}' is not one of low/medium/high"));
    }
    Ok(())
}

/// Strict typed extraction of a proof record, followed by the stack proof rules.
fn validate_proof(r: &Value) -> Result<(), String> {
    let commit = str_field(r, "commit")?;
    if !is_full_sha(commit) {
        return Err("commit is not a 40-char lowercase-hex sha".to_string());
    }
    let changed = str_array_field(r, "changed_surface")?;
    if changed.is_empty() {
        return Err("changed_surface is empty".to_string());
    }
    let blast = str_array_field(r, "blast_radius")?;

    let features = array_field(r, "features")?;
    if features.is_empty() {
        return Err("features is empty".to_string());
    }
    let mut verification_by_key: BTreeMap<String, String> = BTreeMap::new();
    for (i, f) in features.iter().enumerate() {
        object(f).map_err(|e| format!("features[{i}] {e}"))?;
        let key = str_field(f, "key").map_err(|e| format!("features[{i}].{e}"))?;
        let verification =
            str_field(f, "verification").map_err(|e| format!("features[{i}].{e}"))?;
        let verdict = str_field(f, "verdict").map_err(|e| format!("features[{i}].{e}"))?;
        if !matches!(verdict, "pass" | "fail" | "verified-unreachable") {
            return Err(format!(
                "features[{i}].verdict '{verdict}' is not pass/fail/verified-unreachable"
            ));
        }
        if verdict == "fail" {
            return Err(format!("feature '{key}' has verdict fail"));
        }
        if verdict == "verified-unreachable"
            && str_field(f, "unreachable_reason")
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            return Err(format!(
                "feature '{key}' is verified-unreachable without a reason"
            ));
        }
        if let Some(ui) = f.get("ui") {
            object(ui).map_err(|e| format!("feature '{key}' ui {e}"))?;
            if array_field(ui, "screenshots")
                .map_err(|e| format!("feature '{key}' ui.{e}"))?
                .is_empty()
            {
                return Err(format!("feature '{key}' ui has no screenshots"));
            }
            for judgment in ["works", "usable", "looks_good"] {
                if ui.get(judgment).and_then(|v| v.as_bool()) == Some(false) {
                    return Err(format!("feature '{key}' ui judgment '{judgment}' is false"));
                }
            }
        }
        if verification_by_key
            .insert(key.to_string(), verification.to_string())
            .is_some()
        {
            return Err(format!("features[{i}].key '{key}' is a duplicate"));
        }
    }

    let tests = object_field(r, "tests")?;
    let tests_result = str_field(tests, "result").map_err(|e| format!("tests.{e}"))?;
    if !matches!(tests_result, "pass" | "fail") {
        return Err(format!("tests.result '{tests_result}' is not pass/fail"));
    }
    if tests_result != "pass" {
        return Err("tests.result is not pass".to_string());
    }

    let iv = object_field(r, "independent_verifier")?;
    let reran = str_array_field(iv, "reran").map_err(|e| format!("independent_verifier.{e}"))?;
    if !bool_field(iv, "agrees").map_err(|e| format!("independent_verifier.{e}"))? {
        return Err("independent_verifier.agrees is false".to_string());
    }

    // Stack proof rules over the extracted surface.
    for key in changed.iter().chain(blast.iter()) {
        match verification_by_key.get(key) {
            None => {
                return Err(format!(
                    "changed/blast feature '{key}' is missing from features"
                ));
            }
            Some(v) if v != "rerun" => {
                return Err(format!(
                    "changed/blast feature '{key}' has verification '{v}', must be rerun"
                ));
            }
            _ => {}
        }
        if !reran.iter().any(|r| r == key) {
            return Err(format!("independent_verifier did not rerun '{key}'"));
        }
    }
    Ok(())
}

// --- typed field accessors: Err names the field ---

fn object(v: &Value) -> Result<(), String> {
    if v.is_object() {
        Ok(())
    } else {
        Err("is not an object".to_string())
    }
}

fn field<'a>(r: &'a Value, name: &str) -> Result<&'a Value, String> {
    r.get(name).ok_or_else(|| format!("{name} is missing"))
}

fn object_field<'a>(r: &'a Value, name: &str) -> Result<&'a Value, String> {
    let v = field(r, name)?;
    if !v.is_object() {
        return Err(format!("{name} is not an object"));
    }
    Ok(v)
}

fn str_field<'a>(r: &'a Value, name: &str) -> Result<&'a str, String> {
    field(r, name)?
        .as_str()
        .ok_or_else(|| format!("{name} is not a string"))
}

fn bool_field(r: &Value, name: &str) -> Result<bool, String> {
    field(r, name)?
        .as_bool()
        .ok_or_else(|| format!("{name} is not a boolean"))
}

fn array_field<'a>(r: &'a Value, name: &str) -> Result<&'a Vec<Value>, String> {
    field(r, name)?
        .as_array()
        .ok_or_else(|| format!("{name} is not an array"))
}

fn str_array_field(r: &Value, name: &str) -> Result<Vec<String>, String> {
    let arr = array_field(r, name)?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        out.push(
            v.as_str()
                .ok_or_else(|| format!("{name}[{i}] is not a string"))?
                .to_string(),
        );
    }
    Ok(out)
}

/// A 40-character lowercase-hex git sha.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Standard RFC 4648 base64 decode (with `+` / `/` and `=` padding). Returns
/// `None` on any non-alphabet character. Input is expected whitespace-free.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn sextet(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'=' {
        end -= 1;
    }
    let data = &bytes[..end];
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    for &b in data {
        acc = (acc << 6) | sextet(b)?;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_480() -> ParsedRecord {
        parse_record(include_str!("../tests/fixtures/pr480_review.txt"))
    }
    fn review_477() -> ParsedRecord {
        parse_record(include_str!("../tests/fixtures/pr477_review.txt"))
    }
    fn proof_480() -> ParsedRecord {
        parse_record(include_str!("../tests/fixtures/pr480_proof.txt"))
    }
    fn proof_477() -> ParsedRecord {
        parse_record(include_str!("../tests/fixtures/pr477_proof.txt"))
    }

    // --- the real records decode and validate ---

    #[test]
    fn pr480_review_decodes_to_the_comment_record() {
        let p = review_480();
        assert_eq!(p.kind, "review");
        assert!(p.parse_ok, "reason: {}", p.reason);
        assert_eq!(p.commit, "27a90bf7c6971263ea9858861d95f58d27e933f5");
        assert_eq!(p.record["reviewers_ran"], json!(["grok", "codex", "fable"]));
    }

    #[test]
    fn pr480_proof_decodes_and_passes_the_proof_rules() {
        let p = proof_480();
        assert_eq!(p.kind, "proof");
        assert!(p.parse_ok, "reason: {}", p.reason);
        assert_eq!(p.commit, "27a90bf7c6971263ea9858861d95f58d27e933f5");
        assert_eq!(p.record["changed_surface"], json!(["boot-and-health"]));
    }

    #[test]
    fn pr477_review_and_proof_decode() {
        assert!(review_477().parse_ok, "{}", review_477().reason);
        let p = proof_477();
        assert!(p.parse_ok, "reason: {}", p.reason);
        assert_eq!(p.record["blast_radius"], json!(["boot-and-health"]));
    }

    #[test]
    fn malformed_marker_is_not_parse_ok() {
        let p = parse_record(include_str!("../tests/fixtures/malformed_review.txt"));
        assert_eq!(p.kind, "review");
        assert!(!p.parse_ok);
        assert_eq!(p.commit, "");
        assert!(p.reason.contains("base64"), "reason: {}", p.reason);
    }

    #[test]
    fn no_marker_is_none() {
        let p = parse_record(include_str!("../tests/fixtures/no_marker.txt"));
        assert_eq!(p.kind, "none");
        assert!(!p.parse_ok);
    }

    // --- the no-op signal is EXACTLY the empty action (kernel dispatches nothing) ---

    #[test]
    fn no_record_produces_the_empty_no_dispatch_action() {
        assert_eq!(NO_DISPATCH, "");
        let none = parse_record(include_str!("../tests/fixtures/no_marker.txt"));
        assert_eq!(ingest_action(REVIEW_ENTITY, &none), "");
        let malformed = parse_record(include_str!("../tests/fixtures/malformed_review.txt"));
        assert_eq!(ingest_action(REVIEW_ENTITY, &malformed), "");
        // A valid record fed to the wrong entity also writes nothing.
        assert_eq!(ingest_action(PROOF_ENTITY, &review_480()), "");
        assert_eq!(ingest_action(REVIEW_ENTITY, &proof_480()), "");
    }

    #[test]
    fn ingest_action_routes_valid_records() {
        assert_eq!(ingest_action(REVIEW_ENTITY, &review_480()), "IngestRecord");
        assert_eq!(ingest_action(PROOF_ENTITY, &proof_480()), "IngestProof");
    }

    // --- write params + derivation ---

    #[test]
    fn review_write_params_carry_the_ingestrecord_fields() {
        let params = write_params(&review_477());
        assert_eq!(
            params["commit"],
            json!("a8dba3452a498f271a985098eab77467403bbab7")
        );
        assert_eq!(
            params["reviewers_ran"],
            json!("[\"grok\",\"codex\",\"fable\"]")
        );
        assert_eq!(params["risk"], json!("high"));
        assert_eq!(params["open_act_on_count"], json!("1")); // one unresolved act-on
    }

    #[test]
    fn proof_write_params_carry_the_ingestproof_fields() {
        let params = write_params(&proof_480());
        assert_eq!(params["changed_surface"], json!("[\"boot-and-health\"]"));
        assert!(params["tests"].as_str().unwrap().contains("result"));
        assert!(
            params["independent_verifier"]
                .as_str()
                .unwrap()
                .contains("agrees")
        );
    }

    #[test]
    fn open_act_on_count_matches_real_records_and_derivation() {
        assert_eq!(open_act_on_count(&review_480().record), 0);
        assert_eq!(open_act_on_count(&review_477().record), 1);
        let synthetic = json!({ "findings": [
            { "severity": "act-on", "resolved": false },
            { "severity": "act-on", "resolved": true },
            { "severity": "act-on" },
            { "severity": "consider", "resolved": false },
        ] });
        assert_eq!(open_act_on_count(&synthetic), 2);
    }

    // --- strict review extraction: one rejection per class, reason names field ---

    fn good_review() -> Value {
        json!({
            "commit": "27a90bf7c6971263ea9858861d95f58d27e933f5",
            "reviewers_ran": ["fable"],
            "findings": [{ "severity": "act-on", "file_line": "a.rs:1", "resolved": false }],
            "risk": "low"
        })
    }

    #[test]
    fn good_review_validates() {
        assert_eq!(validate_review(&good_review()), Ok(()));
    }

    #[test]
    fn review_rejections_name_the_field() {
        let mut r = good_review();
        r.as_object_mut().unwrap().remove("commit");
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("commit is missing")
        );

        let mut r = good_review();
        r["commit"] = json!(123);
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("commit is not a string")
        );

        let mut r = good_review();
        r["commit"] = json!("abc"); // right type, wrong shape
        assert!(validate_review(&r).unwrap_err().contains("40-char"));

        let mut r = good_review();
        r["reviewers_ran"] = json!("grok"); // wrong type
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("reviewers_ran is not an array")
        );

        let mut r = good_review();
        r["reviewers_ran"] = json!([]);
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("reviewers_ran is empty")
        );

        let mut r = good_review();
        r["reviewers_ran"] = json!(["grok", 7]); // element wrong type
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("reviewers_ran[1] is not a string")
        );

        let mut r = good_review();
        r["findings"] = json!([{ "file_line": "a.rs:1", "resolved": false }]); // missing severity
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("findings[0].severity is missing")
        );

        let mut r = good_review();
        r["findings"] = json!([{ "severity": "act-on", "file_line": "a.rs:1", "resolved": "no" }]);
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("findings[0].resolved is not a boolean")
        );

        let mut r = good_review();
        r["risk"] = json!("spicy");
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("risk 'spicy' is not one of")
        );

        let mut r = good_review();
        r["findings"] =
            json!([{ "severity": "blocker", "file_line": "a.rs:1", "resolved": false }]);
        assert!(
            validate_review(&r)
                .unwrap_err()
                .contains("findings[0].severity 'blocker' is not act-on/consider/nit")
        );
    }

    // --- strict proof extraction + rules: one rejection per class ---

    fn good_proof() -> Value {
        json!({
            "commit": "27a90bf7c6971263ea9858861d95f58d27e933f5",
            "changed_surface": ["x"],
            "blast_radius": [],
            "features": [{ "key": "x", "verification": "rerun", "verdict": "pass", "steps": [] }],
            "tests": { "result": "pass" },
            "independent_verifier": { "reran": ["x"], "agrees": true }
        })
    }

    #[test]
    fn good_proof_validates() {
        assert_eq!(validate_proof(&good_proof()), Ok(()));
    }

    #[test]
    fn proof_rejections_name_the_field() {
        let mut r = good_proof();
        r["changed_surface"] = json!("x"); // wrong type
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("changed_surface is not an array")
        );

        let mut r = good_proof();
        r["changed_surface"] = json!([]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("changed_surface is empty")
        );

        let mut r = good_proof();
        r["features"] = json!([]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("features is empty")
        );

        let mut r = good_proof();
        r["features"] = json!([{ "verification": "rerun", "verdict": "pass" }]); // missing key
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("features[0].key is missing")
        );

        let mut r = good_proof();
        r["features"] = json!([{ "key": "x", "verification": "rerun", "verdict": "great" }]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("verdict 'great' is not")
        );

        let mut r = good_proof();
        r["features"] = json!([{ "key": "x", "verification": "rerun", "verdict": "fail" }]);
        assert!(validate_proof(&r).unwrap_err().contains("verdict fail"));

        let mut r = good_proof();
        r["features"] = json!([{ "key": "x", "verification": "review", "verdict": "pass" }]);
        assert!(validate_proof(&r).unwrap_err().contains("must be rerun"));

        let mut r = good_proof();
        r["changed_surface"] = json!(["x", "y"]); // y missing from features
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("'y' is missing from features")
        );

        let mut r = good_proof();
        r["tests"] = json!("pass"); // wrong type
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("tests is not an object")
        );

        let mut r = good_proof();
        r["tests"]["result"] = json!("fail");
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("tests.result is not pass")
        );

        let mut r = good_proof();
        r["independent_verifier"]["agrees"] = json!("yes"); // wrong type
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("independent_verifier.agrees is not a boolean")
        );

        let mut r = good_proof();
        r["independent_verifier"]["agrees"] = json!(false);
        assert!(validate_proof(&r).unwrap_err().contains("agrees is false"));

        let mut r = good_proof();
        r["independent_verifier"]["reran"] = json!([]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("did not rerun 'x'")
        );

        let mut r = good_proof();
        r["features"] =
            json!([{ "key": "x", "verification": "rerun", "verdict": "verified-unreachable" }]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("verified-unreachable without a reason")
        );

        let mut r = good_proof();
        r["features"] = json!([{ "key": "x", "verification": "rerun", "verdict": "pass",
            "ui": { "screenshots": ["http://x"], "works": false } }]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("ui judgment 'works' is false")
        );

        let mut r = good_proof();
        r["features"] = json!([
            { "key": "x", "verification": "rerun", "verdict": "pass" },
            { "key": "x", "verification": "rerun", "verdict": "pass" }
        ]);
        assert!(
            validate_proof(&r)
                .unwrap_err()
                .contains("'x' is a duplicate")
        );
    }

    #[test]
    fn base64_decodes_standard_input() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert!(base64_decode("not valid!!!").is_none());
    }
}
