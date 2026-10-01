// Evidence chronology is distinct from forecast horizons and retrieval time.
use serde_json::{Value, json};
pub const CHRONOLOGY: &str = "observation_period dates describe when source observations occurred, not a forecast horizon. Preserve projections and their target dates explicitly as projections in the finding statement; use null for unknown observation dates. publication_date dates the source publication, not the projected event. Preserve calendar precision as YYYY, YYYY-Q1 through YYYY-Q4, YYYY-MM or YYYY-MM-DD; retrieved_at requires YYYY-MM-DD. Do not interpret a fiscal quarter as a calendar quarter unless its calendar boundaries are known; otherwise use null and preserve the fiscal period in the finding statement.";
// Partial precision describes a calendar interval; bounds are for comparison
// only. The caller retains the original source value, never an inferred day.
fn date_bounds(value: &Value, day_required: bool) -> Result<Option<(u32, u32)>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let s = value
        .as_str()
        .ok_or("Evidence date must be a string or null")?;
    let parts: Vec<_> = s.split('-').collect();
    let quarter = parts.len() == 2 && parts[1].starts_with('Q');
    if !(1..=3).contains(&parts.len())
        || (day_required && parts.len() != 3)
        || parts[0].len() != 4
        || !parts[0].bytes().all(|b| b.is_ascii_digit())
        || parts.iter().skip(1).any(|p| p.len() != 2)
        || (!quarter
            && parts
                .iter()
                .skip(1)
                .any(|p| !p.bytes().all(|b| b.is_ascii_digit())))
    {
        return Err("Evidence date needs YYYY, YYYY-Q1 through YYYY-Q4, YYYY-MM or YYYY-MM-DD precision; retrieval requires a full day".into());
    }
    let year: u32 = parts[0].parse().map_err(|_| "Invalid evidence year")?;
    if year == 0 {
        return Err("Invalid evidence year".into());
    }
    let (first_month, last_month) = if quarter {
        let q: u32 = parts[1][1..]
            .parse()
            .map_err(|_| "Invalid evidence quarter")?;
        if !(1..=4).contains(&q) {
            return Err("Invalid evidence quarter".into());
        }
        ((q - 1) * 3 + 1, q * 3)
    } else if let Some(month) = parts.get(1) {
        let month: u32 = month.parse().map_err(|_| "Invalid evidence month")?;
        if !(1..=12).contains(&month) {
            return Err("Invalid evidence month".into());
        }
        (month, month)
    } else {
        (1, 12)
    };
    let last_day = match last_month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let (first_day, last_day) = if let Some(day) = parts.get(2) {
        let day: u32 = day.parse().map_err(|_| "Invalid evidence day")?;
        if day == 0 || day > last_day {
            return Err("Invalid evidence calendar date".into());
        }
        (day, day)
    } else {
        (1, last_day)
    };
    Ok(Some((
        year * 10000 + first_month * 100 + first_day,
        year * 10000 + last_month * 100 + last_day,
    )))
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
    date_bounds(&value["publication_date"], false)?;
    date_bounds(&value["retrieved_at"], true)?;
    let period = value["observation_period"]
        .as_object()
        .ok_or("Missing observation period")?;
    if period.len() != 2 || !period.contains_key("start") || !period.contains_key("end") {
        return Err("Observation period needs start and end (unknown may be null)".into());
    }
    let start = date_bounds(&period["start"], false)?;
    let end = date_bounds(&period["end"], false)?;
    if let (Some((start, _)), Some((_, end))) = (start, end)
        && start > end
    {
        return Err("Observation period is reversed".into());
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
    let (vantage_day, _) = date_bounds(&json!(vantage), true)?.ok_or("Missing vantage date")?;
    for (field, value) in [
        ("publication_date", &metadata["publication_date"]),
        (
            "observation_period.start",
            &metadata["observation_period"]["start"],
        ),
        (
            "observation_period.end",
            &metadata["observation_period"]["end"],
        ),
    ] {
        if let Some((first_day, _)) = date_bounds(value, false)?
            && first_day > vantage_day
        {
            return Err(format!(
                "evidence_metadata.{field}={} is later than baseline.as_of={vantage}. {CHRONOLOGY}",
                value.as_str().unwrap_or_default()
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_music_quarters_preserve_source_precision() {
        for start in ["2024-Q2", "2023-Q1"] {
            let metadata = json!({"kind":"finding","observation_period":{"end":"2025-Q1","start":start},"publication_date":"2025-06-10","retrieved_at":"2026-10-01"});
            assert_eq!(parse(&metadata.to_string()).unwrap(), metadata);
            within_vantage(&metadata, "2026-10-01").unwrap();
        }
    }

    #[test]
    fn mixed_precision_intervals_reject_only_wholly_reversed_or_later_dates() {
        for (start, end, valid) in [
            ("2024-Q2", "2024-03-31", false),
            ("2024-Q2", "2024-04", true),
            ("2024-06-30", "2024-Q2", true),
            ("2024-07-01", "2024-Q2", false),
            ("2024-Q4", "2024", true),
            ("2025", "2024-Q4", false),
            ("2024-Q3", "2024-Q2", false),
        ] {
            let metadata = json!({"kind":"finding","observation_period":{"start":start,"end":end},"publication_date":null,"retrieved_at":null});
            assert_eq!(validate(&metadata).is_ok(), valid, "{start} to {end}");
        }
        for field in ["publication_date", "start", "end"] {
            let mut metadata =
                json!({"publication_date":null,"observation_period":{"start":null,"end":null}});
            if field == "publication_date" {
                metadata[field] = json!("2024-Q2");
            } else {
                metadata["observation_period"][field] = json!("2024-Q2");
            }
            assert!(within_vantage(&metadata, "2024-03-31").is_err());
            // A frozen hindcast may use overlapping partial precision, but not
            // a wholly later interval. Retrieval time is not observation time.
            within_vantage(&metadata, "2024-04-01").unwrap();
            within_vantage(&metadata, "2024-05-15").unwrap();
        }
        assert_eq!(
            date_bounds(&json!("2024-02"), false).unwrap(),
            Some((20240201, 20240229))
        );
        assert_eq!(
            date_bounds(&json!("2023-Q1"), false).unwrap(),
            Some((20230101, 20230331))
        );
        for invalid in [
            "2024-Q0",
            "2024-Q5",
            "2024-Q01",
            "2024-Qx",
            "2024-q1",
            "0000-Q1",
            "2024-Q1-01",
            "2023-02-29",
        ] {
            assert!(date_bounds(&json!(invalid), false).is_err(), "{invalid}");
        }
        assert!(date_bounds(&json!("2024-Q2"), true).is_err());
        assert!(within_vantage(&json!({}), "2024-Q2").is_err());
    }

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
