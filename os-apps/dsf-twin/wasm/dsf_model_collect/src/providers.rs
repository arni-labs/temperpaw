//! Provider response parsing retains only operational evidence.
use super::{Coverage, Facts, facts, text_at};
use serde_json::{Value, json};

pub(super) fn github(response: &Value) -> Result<Facts, String> {
    let sha = text_at(response, "/sha")?;
    if ![40, 64].contains(&sha.len())
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid GitHub commit revision".into());
    }
    Ok(facts(
        Coverage::Measured,
        "commit_resolved",
        sha,
        json!({"sha":sha,"commit_date":response.pointer("/commit/committer/date"),"tree_sha":response.pointer("/commit/tree/sha")}),
    ))
}

pub(super) fn datadog(
    response: &Value,
    now_ms: i64,
    window_seconds: &u64,
    max_age_seconds: &u64,
) -> Result<Facts, String> {
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        return Err("Datadog metric query failed".into());
    }
    let series = response
        .get("series")
        .and_then(Value::as_array)
        .ok_or("Datadog series missing")?;
    if series.len() > 100 {
        return Err("Datadog series limit exceeded".into());
    }
    let mut latest = None;
    let mut count = 0usize;
    let mut samples = Vec::new();
    for series in series {
        let points = series
            .get("pointlist")
            .and_then(Value::as_array)
            .ok_or("Datadog pointlist missing")?;
        if points.len() > 3600 {
            return Err("Datadog point limit exceeded".into());
        }
        let mut series_latest: Option<(i64, f64)> = None;
        let mut series_count = 0;
        for point in points {
            let point = point
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or("invalid Datadog point")?;
            let at = point[0]
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or("invalid Datadog point timestamp")? as i64;
            if at < now_ms - *window_seconds as i64 * 1000 || at > now_ms {
                continue;
            }
            if let Some(value) = point[1].as_f64().filter(|n| n.is_finite()) {
                latest = Some(latest.map_or(at, |old: i64| old.max(at)));
                count += 1;
                series_count += 1;
                if series_latest.is_none_or(|(previous, _)| at > previous) {
                    series_latest = Some((at, value));
                }
            } else if !point[1].is_null() {
                return Err("invalid Datadog point value".into());
            }
        }
        // Bound stored evidence; preserve exact latest numeric point per returned series.
        if let Some((at, value)) = series_latest {
            samples.push(json!({"metric":series.get("metric"),"scope":series.get("scope"),"latest_point":[at,value],"numeric_point_count":series_count}));
        }
    }
    let coverage = match latest {
        None => Coverage::Absent,
        Some(at) if now_ms - at > *max_age_seconds as i64 * 1000 => Coverage::Stale,
        Some(_) => Coverage::Measured,
    };
    let outcome = match coverage {
        Coverage::Absent => "no_numeric_points",
        Coverage::Stale => "stale_numeric_points",
        _ => "numeric_points_present",
    };
    Ok(Facts {
        coverage,
        outcome: outcome.into(),
        revision: String::new(),
        facts: json!({"series_returned":series.len(),"numeric_point_count":count,"latest_at_ms":latest,"series":samples}),
        sample_kind: "metric_timeseries",
    })
}
