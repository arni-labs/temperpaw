//! Deterministic bounded logistic calibration and temporal evaluation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_EXAMPLES: usize = 512;
pub const MIN_TRAIN: usize = 12;
pub const MIN_VALIDATION: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub version: String,
    pub mode: String,
    pub slope: f64,
    pub intercept: f64,
    #[serde(default)]
    pub evaluated_through: String,
    #[serde(default)]
    pub used_event_ids: Vec<String>,
}
impl Model {
    pub fn identity(mode: &str) -> Self {
        Self {
            version: "identity-v1".into(),
            mode: mode.into(),
            slope: 1.0,
            intercept: 0.0,
            evaluated_through: String::new(),
            used_event_ids: vec![],
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.slope.is_finite()
            || !self.intercept.is_finite()
            || !(0.05..=4.0).contains(&self.slope)
            || !(-4.0..=4.0).contains(&self.intercept)
            || self.version.is_empty()
            || self.used_event_ids.len() > MAX_EXAMPLES
            || !valid_mode(&self.mode)
        {
            return Err("invalid model parameters or lineage".into());
        }
        if !self.evaluated_through.is_empty() && !valid_time(&self.evaluated_through) {
            return Err("invalid model evaluation time".into());
        }
        Ok(())
    }
    pub fn predict(&self, probability: f64) -> f64 {
        let p = probability.clamp(0.0001, 0.9999);
        1.0 / (1.0 + (-(self.slope * (p / (1.0 - p)).ln() + self.intercept)).exp())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Example {
    pub event_id: String,
    pub base_probability: f64,
    pub outcome: u8,
    pub registered_at: String,
    pub resolved_at: String,
    pub evidence_kind: String,
    pub source_refs: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Prepared {
    pub training: Vec<Example>,
    pub validation: Vec<Example>,
    pub skipped_reasons: BTreeMap<String, usize>,
}
pub fn valid_mode(mode: &str) -> bool {
    matches!(mode, "observed" | "historical" | "simulated")
}
/// Require fixed-width UTC seconds so chronological comparisons are unambiguous.
pub fn valid_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return false;
    }
    if bytes
        .iter()
        .enumerate()
        .any(|(i, b)| ![4, 7, 10, 13, 16, 19].contains(&i) && !b.is_ascii_digit())
    {
        return false;
    }
    let num = |a, b| value[a..b].parse::<u32>().unwrap_or(0);
    let year = num(0, 4);
    let month = num(5, 7);
    let day = num(8, 10);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days && num(11, 13) < 24 && num(14, 16) < 60 && num(17, 19) < 60
}
fn invalid_example(e: &Example, mode: &str, as_of: &str) -> Option<&'static str> {
    if e.event_id.is_empty() || e.event_id.len() > 256 {
        return Some("missing_event_identity");
    }
    if !e.base_probability.is_finite()
        || !(0.0..=1.0).contains(&e.base_probability)
        || e.outcome > 1
    {
        return Some("invalid_probability_or_outcome");
    }
    if !valid_time(&e.registered_at)
        || !valid_time(&e.resolved_at)
        || e.registered_at >= e.resolved_at
    {
        return Some("invalid_or_leaked_time");
    }
    if e.resolved_at.as_str() > as_of {
        return Some("not_yet_resolved");
    }
    if e.evidence_kind != mode {
        return Some("wrong_provenance");
    }
    if e.source_refs.is_empty()
        || e.source_refs.len() > 16
        || e.source_refs.iter().any(|s| s.is_empty() || s.len() > 2048)
    {
        return Some("missing_or_invalid_sources");
    }
    if mode != "simulated"
        && e.source_refs.iter().any(|s| {
            !s.starts_with("https://")
                && !(mode == "historical" && s.starts_with("file:") && s.len() > 5)
        })
    {
        return Some("unsupported_source");
    }
    None
}
pub fn prepare(
    mut data: Vec<Example>,
    mode: &str,
    as_of: &str,
    incumbent: &Model,
) -> Result<Prepared, String> {
    if !valid_mode(mode) || !valid_time(as_of) {
        return Err("mode and as_of must be valid; use UTC seconds".into());
    }
    incumbent.validate()?;
    if incumbent.mode != mode {
        return Err("run and model provenance must match".into());
    }
    if data.len() > MAX_EXAMPLES {
        return Err("dataset exceeds 512 examples; no truncation is permitted".into());
    }
    let mut outcomes: BTreeMap<String, u8> = BTreeMap::new();
    let mut conflicts = BTreeSet::new();
    for example in &data {
        if invalid_example(example, mode, as_of).is_none()
            && let Some(previous) = outcomes.insert(example.event_id.clone(), example.outcome)
            && previous != example.outcome
        {
            conflicts.insert(example.event_id.clone());
        }
    }
    data.sort_by(|a, b| (&a.registered_at, &a.event_id).cmp(&(&b.registered_at, &b.event_id)));
    let mut skipped = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut valid = Vec::new();
    for example in data {
        let reason = if incumbent.used_event_ids.contains(&example.event_id) {
            Some("previously_used_event")
        } else if conflicts.contains(&example.event_id) {
            Some("conflicting_event_outcomes")
        } else {
            invalid_example(&example, mode, as_of)
        }
        .or_else(|| {
            if !seen.insert(example.event_id.clone()) {
                Some("duplicate_event")
            } else {
                None
            }
        });
        if let Some(reason) = reason {
            *skipped.entry(reason.into()).or_insert(0) += 1;
        } else {
            valid.push(example);
        }
    }
    valid.sort_by(|a, b| (&a.resolved_at, &a.event_id).cmp(&(&b.resolved_at, &b.event_id)));
    let split = valid.len() * 2 / 3;
    let cutoff = valid[..split]
        .last()
        .map(|e| e.resolved_at.clone())
        .unwrap_or_default();
    let mut training = Vec::new();
    let mut validation = Vec::new();
    for (index, example) in valid.into_iter().enumerate() {
        if index < split {
            training.push(example);
        } else if example.registered_at > cutoff
            && example.registered_at > incumbent.evaluated_through
        {
            validation.push(example);
        } else {
            *skipped
                .entry("validation_not_prospective".into())
                .or_insert(0) += 1;
        }
    }
    if incumbent.used_event_ids.len() + training.len() + validation.len() > MAX_EXAMPLES {
        return Err("Cumulative model lineage would exceed 512 events; incumbent retained, no history discarded".into());
    }
    Ok(Prepared {
        training,
        validation,
        skipped_reasons: skipped,
    })
}
pub fn fit(prepared: &Prepared, incumbent: &Model, version: &str) -> Model {
    let mut candidate = incumbent.clone();
    candidate.version = version.into();
    if prepared.training.len() >= MIN_TRAIN {
        for _ in 0..500 {
            let mut slope_gradient = 0.0;
            let mut intercept_gradient = 0.0;
            for example in &prepared.training {
                let p = example.base_probability.clamp(0.0001, 0.9999);
                let residual = candidate.predict(p) - f64::from(example.outcome);
                slope_gradient += residual * (p / (1.0 - p)).ln();
                intercept_gradient += residual;
            }
            let count = prepared.training.len() as f64;
            candidate.slope = (candidate.slope - 0.1 * slope_gradient / count).clamp(0.05, 4.0);
            candidate.intercept =
                (candidate.intercept - 0.1 * intercept_gradient / count).clamp(-4.0, 4.0);
        }
    }
    candidate.evaluated_through = prepared
        .validation
        .iter()
        .map(|e| e.resolved_at.clone())
        .max()
        .unwrap_or_else(|| incumbent.evaluated_through.clone());
    candidate.used_event_ids = incumbent
        .used_event_ids
        .iter()
        .cloned()
        .chain(
            prepared
                .training
                .iter()
                .chain(&prepared.validation)
                .map(|e| e.event_id.clone()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    candidate
}
pub fn evaluate(
    prepared: &Prepared,
    incumbent: &Model,
    candidate: &Model,
    mode: &str,
    as_of: &str,
) -> serde_json::Value {
    let training_count = prepared.training.len();
    let validation_count = prepared.validation.len();
    let enough = training_count >= MIN_TRAIN && validation_count >= MIN_VALIDATION;
    let mut old_loss = 0.0;
    let mut new_loss = 0.0;
    let mut gains = Vec::new();
    for example in &prepared.validation {
        let old =
            (incumbent.predict(example.base_probability) - f64::from(example.outcome)).powi(2);
        let new =
            (candidate.predict(example.base_probability) - f64::from(example.outcome)).powi(2);
        old_loss += old;
        new_loss += new;
        gains.push(old - new);
    }
    let count = validation_count.max(1) as f64;
    let improvement = (old_loss - new_loss) / count;
    let variance =
        gains.iter().map(|g| (g - improvement).powi(2)).sum::<f64>() / (count - 1.0).max(1.0);
    let standard_error = (variance / count).sqrt();
    let adopt = enough && improvement >= 0.005 && improvement > 2.0 * standard_error;
    let reason = if !enough {
        "Insufficient temporally separate evidence: need 12 training and 8 new validation events."
    } else if adopt {
        "Candidate improves held-out Brier score by at least 0.005 and more than two paired standard errors."
    } else {
        "Candidate did not clear the held-out improvement threshold; incumbent retained."
    };
    serde_json::json!({
        "schema_version":1, "decision":if adopt {"candidate_passed"} else {"rejected"}, "reason":reason,
        "mode":mode,"as_of":as_of,"training_count":training_count,"validation_count":validation_count,
        "skipped_count":prepared.skipped_reasons.values().sum::<usize>(), "skipped_reasons":prepared.skipped_reasons,
        "incumbent_brier":if validation_count>0 {Some(old_loss/count)} else {None},
        "candidate_brier":if validation_count>0 {Some(new_loss/count)} else {None},
        "improvement":improvement,"paired_standard_error":standard_error,
        "training_ids":prepared.training.iter().map(|e|&e.event_id).collect::<Vec<_>>(),
        "validation_ids":prepared.validation.iter().map(|e|&e.event_id).collect::<Vec<_>>(),
        "learned_changes":[
            {"name":"slope","before":incumbent.slope,"after":candidate.slope,"meaning":"How strongly input probabilities distinguish outcomes."},
            {"name":"intercept","before":incumbent.intercept,"after":candidate.intercept,"meaning":"Correction to systematic over- or underprediction."}],
        "provenance_summary":format!("{} dated {} examples; modes never mix",training_count+validation_count,mode),
        "evaluation_limitations":[
            "A small held-out comparison is not proof of future accuracy or a causal world model.",
            "Source links and timestamps are supplied evidence; this learner does not independently verify their truth.",
            "Historical replay may retain base-model knowledge of later events.",
            if mode=="simulated" {"Synthetic mechanics demonstration; no empirical quality claim."} else {"Future observed outcomes are the next independent check."}]
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<Example> {
        (0..36)
            .map(|i| Example {
                event_id: format!("event-{i}"),
                base_probability: if i % 2 == 0 { 0.55 } else { 0.45 },
                outcome: if i % 2 == 0 { 1 } else { 0 },
                registered_at: format!("2025-{:02}-{:02}T00:00:00Z", i / 20 + 1, i % 20 + 1),
                resolved_at: format!("2025-{:02}-{:02}T01:00:00Z", i / 20 + 1, i % 20 + 1),
                evidence_kind: "simulated".into(),
                source_refs: vec!["fixture:calibration".into()],
            })
            .collect()
    }

    #[test]
    fn shuffled_duplicate_and_delayed_feedback_replays_identically() {
        let model = Model::identity("simulated");
        let mut baseline = fixture();
        baseline.push(baseline[3].clone());
        let mut delayed = baseline[5].clone();
        delayed.event_id = "delayed".into();
        delayed.resolved_at = "2027-01-01T00:00:00Z".into();
        baseline.push(delayed);
        let expected = prepare(
            baseline.clone(),
            "simulated",
            "2025-12-31T00:00:00Z",
            &model,
        )
        .unwrap();
        for seed in 1..=64u64 {
            let mut schedule = seed;
            let mut reordered = baseline.clone();
            for i in (1..reordered.len()).rev() {
                schedule = schedule.wrapping_mul(6364136223846793005).wrapping_add(1);
                reordered.swap(i, (schedule as usize) % (i + 1));
            }
            let actual = prepare(reordered, "simulated", "2025-12-31T00:00:00Z", &model).unwrap();
            assert_eq!(actual, expected, "seed {seed}");
            assert_eq!(
                fit(&actual, &model, "candidate"),
                fit(&expected, &model, "candidate"),
                "seed {seed}"
            );
        }
    }
    #[test]
    fn contradictory_truth_is_excluded_in_every_order() {
        let mut data = fixture();
        let mut contradiction = data[0].clone();
        contradiction.outcome = 1 - contradiction.outcome;
        data.push(contradiction);
        let prepared = prepare(
            data,
            "simulated",
            "2025-12-31T00:00:00Z",
            &Model::identity("simulated"),
        )
        .unwrap();
        assert_eq!(prepared.skipped_reasons["conflicting_event_outcomes"], 2);
        assert!(
            prepared
                .training
                .iter()
                .chain(&prepared.validation)
                .all(|e| e.event_id != "event-0")
        );
    }
    #[test]
    fn adopted_events_are_never_retrained_and_lineage_accumulates() {
        let identity = Model::identity("simulated");
        let first = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &identity).unwrap();
        let incumbent = fit(&first, &identity, "first");
        let replay = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &incumbent).unwrap();
        assert!(replay.training.is_empty());
        assert!(replay.validation.is_empty());
        assert_eq!(
            replay.skipped_reasons.get("previously_used_event"),
            Some(&36)
        );
        let fresh: Vec<_> = fixture()
            .into_iter()
            .map(|mut event| {
                event.event_id = format!("fresh-{}", event.event_id);
                event.registered_at = event.registered_at.replace("2025", "2026");
                event.resolved_at = event.resolved_at.replace("2025", "2026");
                event
            })
            .collect();
        let mut combined = fixture();
        combined.extend(fresh);
        let next = prepare(combined, "simulated", "2026-12-31T00:00:00Z", &incumbent).unwrap();
        assert_eq!(next.training.len(), 24);
        assert_eq!(next.validation.len(), 12);
        let candidate = fit(&next, &incumbent, "next");
        assert_eq!(candidate.used_event_ids.len(), 72);
        assert!(
            incumbent
                .used_event_ids
                .iter()
                .all(|id| candidate.used_event_ids.contains(id))
        );
        assert!(candidate.validate().is_ok());
        let mut third_input: Vec<_> = fixture().into_iter().take(10).collect();
        third_input.extend(fixture().into_iter().map(|mut e| {
            e.event_id = format!("third-{}", e.event_id);
            e.registered_at = e.registered_at.replace("2025", "2027");
            e.resolved_at = e.resolved_at.replace("2025", "2027");
            e
        }));
        let third = prepare(third_input, "simulated", "2027-12-31T00:00:00Z", &candidate).unwrap();
        assert_eq!(
            third.skipped_reasons.get("previously_used_event"),
            Some(&10)
        );
        let third_model = fit(&third, &candidate, "third");
        assert_eq!(third_model.used_event_ids.len(), 108);
        assert!(
            candidate
                .used_event_ids
                .iter()
                .all(|id| third_model.used_event_ids.contains(id))
        );
    }

    #[test]
    fn cumulative_lineage_bound_fails_without_forgetting_history() {
        let mut incumbent = Model::identity("simulated");
        incumbent.used_event_ids = (0..MAX_EXAMPLES).map(|i| format!("used-{i}")).collect();
        let before = incumbent.clone();
        let result = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &incumbent);
        assert!(result.is_err());
        assert_eq!(incumbent, before);
    }

    #[test]
    fn actual_fitting_changes_parameters_and_later_prediction() {
        let incumbent = Model::identity("simulated");
        let prepared = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &incumbent).unwrap();
        let candidate = fit(&prepared, &incumbent, "run-one");
        assert!(candidate.predict(0.55) > incumbent.predict(0.55) + 0.05);
        assert_eq!(
            evaluate(
                &prepared,
                &incumbent,
                &candidate,
                "simulated",
                "2025-12-31T00:00:00Z"
            )["decision"],
            "candidate_passed"
        );
        assert_eq!(incumbent.slope, 1.0);
    }
    #[test]
    fn one_fresh_event_is_validation_even_when_revisions_repeat_it() {
        let mut incumbent = Model::identity("simulated");
        incumbent.evaluated_through = "2025-02-16T00:00:00Z".into();
        let mut first = fixture()[0].clone();
        first.registered_at = "2025-03-01T00:00:00Z".into();
        first.resolved_at = "2025-06-01T00:00:00Z".into();
        let mut revision = first.clone();
        revision.registered_at = "2025-03-02T00:00:00Z".into();
        for data in [vec![first.clone()], vec![revision, first.clone()]] {
            let duplicates = data.len() - 1;
            let prepared = prepare(data, "simulated", "2025-06-01T00:00:00Z", &incumbent).unwrap();
            assert!(prepared.training.is_empty());
            assert_eq!(prepared.validation, vec![first.clone()]);
            assert_eq!(
                prepared
                    .skipped_reasons
                    .get("duplicate_event")
                    .copied()
                    .unwrap_or(0),
                duplicates
            );
            assert!(
                !prepared
                    .skipped_reasons
                    .contains_key("validation_not_prospective")
            );
            let candidate = fit(&prepared, &incumbent, "insufficient-run");
            let report = evaluate(
                &prepared,
                &incumbent,
                &candidate,
                "simulated",
                "2025-06-01T00:00:00Z",
            );
            assert_eq!(report["training_count"], 0);
            assert_eq!(report["validation_count"], 1);
            assert_eq!(report["decision"], "rejected");
            assert_eq!(candidate.predict(0.55), incumbent.predict(0.55));
        }
    }

    #[test]
    fn temporal_separation_and_duplicate_events() {
        let mut data = fixture();
        data.push(data[0].clone());
        let prepared = prepare(
            data,
            "simulated",
            "2025-12-31T00:00:00Z",
            &Model::identity("simulated"),
        )
        .unwrap();
        assert_eq!(prepared.skipped_reasons["duplicate_event"], 1);
        let latest = prepared
            .training
            .iter()
            .map(|e| &e.resolved_at)
            .max()
            .unwrap();
        assert!(
            prepared
                .validation
                .iter()
                .all(|e| e.registered_at > *latest)
        );
        assert!(
            prepared
                .training
                .iter()
                .all(|a| prepared.validation.iter().all(|b| a.event_id != b.event_id))
        );
    }
    #[test]
    fn rejects_leaked_future_and_wrong_provenance() {
        let mut data = fixture();
        data[0].registered_at = data[0].resolved_at.clone();
        data[1].resolved_at = "2027-01-01T00:00:00Z".into();
        data[2].evidence_kind = "observed".into();
        let prepared = prepare(
            data,
            "simulated",
            "2025-12-31T00:00:00Z",
            &Model::identity("simulated"),
        )
        .unwrap();
        assert_eq!(prepared.skipped_reasons.values().sum::<usize>(), 3);
    }
    #[test]
    fn replay_is_deterministic_and_consumed_validation_cannot_promote_again() {
        let incumbent = Model::identity("simulated");
        let prepared = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &incumbent).unwrap();
        let candidate = fit(&prepared, &incumbent, "run-one");
        assert_eq!(candidate, fit(&prepared, &incumbent, "run-one"));
        let replay = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &candidate).unwrap();
        assert!(replay.validation.is_empty());
    }
    #[test]
    fn insufficient_data_and_unchanged_model_are_rejected() {
        let incumbent = Model::identity("simulated");
        let mut data = fixture();
        data.truncate(6);
        let small = prepare(data, "simulated", "2025-12-31T00:00:00Z", &incumbent).unwrap();
        assert_eq!(
            evaluate(
                &small,
                &incumbent,
                &incumbent,
                "simulated",
                "2025-12-31T00:00:00Z"
            )["decision"],
            "rejected"
        );
        let full = prepare(fixture(), "simulated", "2025-12-31T00:00:00Z", &incumbent).unwrap();
        assert_eq!(
            evaluate(
                &full,
                &incumbent,
                &incumbent,
                "simulated",
                "2025-12-31T00:00:00Z"
            )["decision"],
            "rejected"
        );
    }
    #[test]
    fn invalid_dates_and_models_are_rejected() {
        assert!(!valid_time("2025-02-30T00:00:00Z"));
        assert!(!valid_time("2025-01-01"));
        assert!(valid_time("2024-02-29T00:00:00Z"));
        let mut model = Model::identity("observed");
        model.slope = f64::NAN;
        assert!(model.validate().is_err());
        assert!(
            prepare(
                fixture(),
                "observed",
                "2025-12-31T00:00:00Z",
                &Model::identity("simulated")
            )
            .is_err()
        );
    }
}
