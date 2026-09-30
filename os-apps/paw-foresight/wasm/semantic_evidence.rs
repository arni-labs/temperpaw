// Evidence chronology is distinct from forecast horizons and retrieval time.
use serde_json::{Value, json};
fn date(value: &Value, day_required: bool) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let s = value
        .as_str()
        .ok_or("Evidence date must be a string or null")?;
    let parts: Vec<_> = s.split('-').collect();
    if !(1..=3).contains(&parts.len())
        || (day_required && parts.len() != 3)
        || parts[0].len() != 4
        || parts
            .iter()
            .enumerate()
            .any(|(i, p)| (i > 0 && p.len() != 2) || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Evidence date needs YYYY, YYYY-MM or YYYY-MM-DD precision".into());
    }
    let year: u32 = parts[0].parse().map_err(|_| "Invalid evidence year")?;
    if year == 0 {
        return Err("Invalid evidence year".into());
    }
    let month: u32 = parts
        .get(1)
        .map_or(Ok(1), |p| p.parse())
        .map_err(|_| "Invalid evidence month")?;
    if !(1..=12).contains(&month) {
        return Err("Invalid evidence month".into());
    }
    if let Some(day) = parts.get(2) {
        let day: u32 = day.parse().map_err(|_| "Invalid evidence day")?;
        let max = match month {
            2 if year.is_multiple_of(4)
                && (!year.is_multiple_of(100) || year.is_multiple_of(400)) =>
            {
                29
            }
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day == 0 || day > max {
            return Err("Invalid evidence calendar date".into());
        }
    }
    Ok(())
}
pub fn validate(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("Missing structured evidence_metadata")?;
    if object.len() != 4 || !matches!(value["kind"].as_str(), Some("finding" | "lead")) {
        return Err(
            "Evidence metadata needs kind finding or lead and separate chronology fields".into(),
        );
    }
    for key in ["publication_date", "observation_period", "retrieved_at"] {
        if !object.contains_key(key) {
            return Err(format!("Missing evidence {key}"));
        }
    }
    date(&value["publication_date"], false)?;
    date(&value["retrieved_at"], true)?;
    let period = value["observation_period"]
        .as_object()
        .ok_or("Missing observation period")?;
    if period.len() != 2 || !period.contains_key("start") || !period.contains_key("end") {
        return Err("Observation period needs start and end (unknown may be null)".into());
    }
    date(&period["start"], false)?;
    date(&period["end"], false)?;
    if let (Some(start), Some(end)) = (period["start"].as_str(), period["end"].as_str()) {
        // Compare only common precision; a year is an interval, not January 1.
        let n = start.len().min(end.len());
        if start[..n] > end[..n] {
            return Err("Observation period is reversed".into());
        }
    }
    Ok(())
}
pub fn legacy() -> Value {
    json!({"kind":"legacy_unverified","publication_date":null,"observation_period":{"start":null,"end":null},"retrieved_at":null})
}
pub fn parse(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(legacy());
    }
    let value: Value = serde_json::from_str(raw).map_err(|_| "Invalid evidence_json")?;
    validate(&value)?;
    Ok(value)
}

pub fn single_source(refs: &Value) -> Result<(), String> {
    let refs: Value = match refs.as_str() {
        Some(raw) => serde_json::from_str(raw).map_err(|_| "Invalid source_refs")?,
        None => refs.clone(),
    };
    if refs.as_array().is_none_or(|v| {
        v.len() != 1
            || v[0]
                .as_str()
                .is_none_or(|s| s.trim().is_empty() || s.len() > 4000)
    }) {
        return Err("Typed evidence needs exactly one supporting source reference; split different sources into separate findings".into());
    }
    Ok(())
}
pub fn within_vantage(metadata: &Value, vantage: &str) -> Result<(), String> {
    date(&json!(vantage), true)?;
    for value in [
        &metadata["publication_date"],
        &metadata["observation_period"]["start"],
        &metadata["observation_period"]["end"],
    ] {
        if let Some(date) = value.as_str() {
            // Partial dates denote intervals. Reject only a wholly later interval.
            let n = date.len().min(vantage.len());
            if date[..n] > vantage[..n] {
                return Err(
                    "Evidence publication or observation is later than the baseline vantage".into(),
                );
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chronology_keeps_precision_unknowns_and_legacy_distinct() {
        let metadata = json!({"kind":"finding","publication_date":"2025","observation_period":{"start":"2020-10","end":"2021-01"},"retrieved_at":"2026-09-30"});
        assert_eq!(parse(&metadata.to_string()).unwrap(), metadata);
        assert_eq!(parse("").unwrap()["kind"], "legacy_unverified");
        let unknown = json!({"kind":"finding","publication_date":null,"observation_period":{"start":null,"end":null},"retrieved_at":null});
        assert_eq!(parse(&unknown.to_string()).unwrap(), unknown);
        for bad in ["2026-02-30", "2026-13", "2026-9", "0000"] {
            let mut m = metadata.clone();
            m["publication_date"] = json!(bad);
            assert!(validate(&m).is_err());
        }
        let mut bad = metadata.clone();
        bad["observation_period"]["start"] = json!("2022");
        assert!(validate(&bad).is_err());
    }
}
