// Producer and consumer share these distinct judgments; neither certifies coverage.
use serde_json::{Value, json};
pub const SCOPE_TEXT_MAX: usize = 800;
pub const LIMITATIONS_MAX: usize = 16;
pub const LIMITATION_TEXT_MAX: usize = 240;
pub const REPORT_MAX: usize = 1200;
pub const REPORT_REFS_MAX: usize = 32;
const REVIEW_STATUS: &[&str] = &["aligned", "narrowed", "uncertain"];
const NARROWING_BASIS: &[&str] = &["user_explicit", "evidence_availability", "none"];
const DISPOSITION_STATUS: &[&str] = &["addressed", "limited", "uncertain"];

pub fn contract() -> Value {
    json!({
        "scope_review": {
            "requested_question":"exact original world.description",
            "evidence_scope":{"minLength":1,"maxLength":SCOPE_TEXT_MAX},
            "status":{"enum":REVIEW_STATUS},
            "narrowing_basis":{"enum":NARROWING_BASIS},
            "limitations":{"minItems":0,"maxItems":LIMITATIONS_MAX,"itemMinLength":1,"itemMaxLength":LIMITATION_TEXT_MAX,"condition":"nonempty unless aligned; copy unresolved limitations into baseline.unknowns"}
        },
        "scope_disposition": {
            "status":{"enum":DISPOSITION_STATUS},
            "report":{"minLength":1,"maxLength":REPORT_MAX},
            "evidence_ids":{"minItems":0,"maxItems":REPORT_REFS_MAX,"items":"actual existing evidence refs or new research_evidence local IDs; addressed requires at least one typed finding"}
        }
    })
}
pub fn validate_review(value: &Value) -> Result<(), String> {
    validate(value, "scope_review", "status", REVIEW_STATUS)?;
    validate(value, "scope_review", "narrowing_basis", NARROWING_BASIS)
}
pub fn validate_disposition(value: &Value) -> Result<(), String> {
    validate(value, "scope_disposition", "status", DISPOSITION_STATUS)
}
fn validate(value: &Value, object: &str, field: &str, allowed: &[&str]) -> Result<(), String> {
    if value[field]
        .as_str()
        .is_some_and(|text| allowed.contains(&text))
    {
        Ok(())
    } else {
        Err(format!(
            "Invalid {object}.{field}; accepted values: {}",
            allowed.join(", ")
        ))
    }
}
