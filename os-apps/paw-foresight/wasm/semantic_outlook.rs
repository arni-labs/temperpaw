// Validate presentation probabilities separately from Jev's causal classifiers.
use crate::core::evidence as evidence_contract;
use serde_json::Value;
use std::collections::BTreeSet;

fn text(value: &Value, limit: usize) -> Result<&str, String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= limit)
        .ok_or_else(|| format!("Outlook text must contain 1–{limit} characters"))
}
fn list(value: &Value, min: usize, max: usize, limit: usize) -> Result<(), String> {
    let values = value
        .as_array()
        .filter(|v| (min..=max).contains(&v.len()))
        .ok_or("Invalid outlook list length")?;
    for v in values {
        text(v, limit)?;
    }
    Ok(())
}
/// The modeled scenarios are an explicit finite partition; `other` retains
/// probability mass for real futures outside that deliberately incomplete model.
pub fn validate(answer: &Value, snapshot: &Value) -> Result<(), String> {
    if answer["schema"] == "foresight-worlds-v3" {
        return validate_v3(answer, snapshot);
    }
    if answer["schema"] == "foresight-outlook-v2" {
        return validate_v2(answer, snapshot);
    }
    validate_v1(answer, snapshot)
}
fn validate_v1(answer: &Value, snapshot: &Value) -> Result<(), String> {
    if answer["schema"] != "foresight-outlook-v1"
        || answer["probability_basis"] != "subjective_model_estimate"
        || answer["calibrated"] != false
    {
        return Err(
            "Outlook must label probabilities as uncalibrated subjective model estimates".into(),
        );
    }
    text(&answer["headline"], 160)?;
    text(&answer["summary"], 400)?;
    if answer["horizon"] != snapshot["world"]["target_date"] {
        return Err("Outlook horizon differs from the question".into());
    }
    list(&answer["evidence_limits"], 1, 6, 240)?;
    list(&answer["research_questions"], 0, 8, 240)?;
    let scenarios: BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .ok_or("Missing snapshot nodes")?
        .iter()
        .filter(|n| n["kind"] == "scenario")
        .filter_map(|n| n["Id"].as_str())
        .collect();
    if scenarios.is_empty() {
        return Err("Outlook requires modeled scenarios".into());
    }
    let outcomes = answer["outcomes"]
        .as_array()
        .filter(|v| (3..=5).contains(&v.len()))
        .ok_or("Expected 3–5 outcome buckets")?;
    let mut used = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut total = 0.0;
    let mut other = false;
    for outcome in outcomes {
        let id = text(&outcome["id"], 50)?;
        if !ids.insert(id) {
            return Err("Repeated outcome identity".into());
        }
        text(&outcome["title"], 70)?;
        text(&outcome["definition"], 240)?;
        text(&outcome["narrative"], 360)?;
        list(&outcome["signals"], 1, 3, 160)?;
        list(&outcome["falsifiers"], 1, 3, 160)?;
        let p = outcome["probability"]
            .as_f64()
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .ok_or("Invalid subjective outcome probability")?;
        total += p;
        let refs = outcome["scenario_ids"]
            .as_array()
            .ok_or("Missing scenario membership")?;
        if id == "other" {
            if !refs.is_empty() {
                return Err("Residual other cannot overlap modeled scenarios".into());
            }
            other = true;
        } else {
            if refs.is_empty() {
                return Err("Outcome has no modeled scenarios".into());
            }
            for reference in refs {
                let reference = reference.as_str().ok_or("Invalid scenario reference")?;
                if !scenarios.contains(reference) || !used.insert(reference) {
                    return Err("Scenario membership is invented or overlapping".into());
                }
            }
        }
    }
    if !other || used != scenarios {
        return Err(
            "Outcome buckets must partition modeled scenarios and include residual other".into(),
        );
    }
    if (total - 1.0).abs() > 0.000001 {
        return Err("Subjective outcome probabilities must sum to one".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn leads_and_unverified_legacy_cannot_establish_new_baselines() {
        let baseline = json!({"as_of":"2026-09-30","observed":[{"claim":"A substantive finding","evidence_ids":["e"]}],"assumptions":[],"unknowns":[]});
        let metadata = json!({"kind":"lead","publication_date":"2025","observation_period":{"start":"2020","end":"2021"},"retrieved_at":"2026-09-30"});
        let mut snapshot = json!({"world":{"last_ingest_date":"2026-09-30","evidence_contract":"v1"},"nodes":[{"Id":"e","kind":"evidence","evidence_metadata":metadata}]});
        assert!(
            validate_baseline(&baseline, &snapshot)
                .unwrap_err()
                .contains("lead")
        );
        snapshot["nodes"][0]["evidence_metadata"]["kind"] = json!("finding");
        assert!(validate_baseline(&baseline, &snapshot).is_ok());
        snapshot["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("evidence_metadata");
        assert!(validate_baseline(&baseline, &snapshot).is_err());
        let mut limited = baseline.clone();
        limited["observed"] = json!([]);
        limited["unknowns"] = json!(["Historical source content has not been verified."]);
        assert!(validate_baseline(&limited, &snapshot).is_ok());
        snapshot["world"]
            .as_object_mut()
            .unwrap()
            .remove("evidence_contract");
        assert!(validate_baseline(&baseline, &snapshot).is_ok());
    }
    fn fixture() -> (Value, Value) {
        let outcome = |id: &str, refs: Vec<&str>, probability: f64| json!({"id":id,"title":"A future","definition":"Observable non-overlapping outcome rule","probability":probability,"scenario_ids":refs,"narrative":"Hypothetical outcome, not an observation.","signals":["A dated observable signal"],"falsifiers":["A measurable disconfirmation"]});
        (
            json!({"schema":"foresight-outlook-v1","headline":"Three alternatives","horizon":"2027-09-19","probability_basis":"subjective_model_estimate","calibrated":false,"summary":"A subjective distribution over mutually exclusive buckets, not measured accuracy.","evidence_limits":["Sparse evidence"],"research_questions":["What could change adoption?"],"outcomes":[outcome("a",vec!["s1"],0.5),outcome("b",vec!["s2"],0.35),outcome("other",vec![],0.15)]}),
            json!({"world":{"target_date":"2027-09-19"},"nodes":[{"Id":"s1","kind":"scenario"},{"Id":"s2","kind":"scenario"}]}),
        )
    }
    #[test]
    fn valid_subjective_partition_is_accepted() {
        let (a, s) = fixture();
        assert!(validate(&a, &s).is_ok());
    }
    #[test]
    fn malformed_probability_claims_fail_closed() {
        let (a, s) = fixture();
        for (key, value) in [
            ("calibrated", json!(true)),
            ("probability_basis", json!("jev_distribution")),
            ("horizon", json!("2028-01-01")),
        ] {
            let mut bad = a.clone();
            bad[key] = value;
            assert!(validate(&bad, &s).is_err());
        }
        let mut bad = a;
        bad["outcomes"][0]["probability"] = json!(0.7);
        assert!(validate(&bad, &s).is_err());
    }
    #[test]
    fn overlapping_missing_and_invented_memberships_fail() {
        let (a, s) = fixture();
        for refs in [json!(["s1"]), json!(["imaginary"]), json!([])] {
            let mut bad = a.clone();
            bad["outcomes"][1]["scenario_ids"] = refs;
            assert!(validate(&bad, &s).is_err());
        }
        let mut bad = a;
        bad["outcomes"][2]["id"] = json!("not-other");
        assert!(validate(&bad, &s).is_err());
    }
    #[test]
    fn excessive_or_empty_display_text_is_rejected() {
        let (a, s) = fixture();
        let mut bad = a.clone();
        bad["headline"] = json!("x".repeat(161));
        assert!(validate(&bad, &s).is_err());
        let mut bad = a;
        bad["outcomes"][0]["narrative"] = json!("");
        assert!(validate(&bad, &s).is_err());
    }
}

/// Independent event estimates may overlap and must never be normalized into a partition.
fn validate_v2(answer: &Value, snapshot: &Value) -> Result<(), String> {
    if answer["probability_model"] != "overlapping_events"
        || answer["probability_basis"] != "model_implied_event_estimate"
        || answer["calibrated"] != false
    {
        return Err("Outlook must identify overlapping uncalibrated event estimates".into());
    }
    text(&answer["headline"], 160)?;
    text(&answer["summary"], 400)?;
    if answer["horizon"] != snapshot["world"]["target_date"] {
        return Err("Outlook horizon differs from the question".into());
    }
    list(&answer["evidence_limits"], 1, 32, 240)?;
    list(&answer["research_questions"], 0, 64, 240)?;
    let hypotheses: BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .ok_or("Missing snapshot nodes")?
        .iter()
        .filter(|n| matches!(n["kind"].as_str(), Some("scenario" | "revision")))
        .filter_map(|n| n["Id"].as_str())
        .collect();
    let known_nodes: BTreeSet<_> = snapshot["nodes"]
        .as_array()
        .ok_or("Missing snapshot nodes")?
        .iter()
        .filter_map(|n| n["Id"].as_str())
        .collect();
    let outcomes = answer["outcomes"]
        .as_array()
        .filter(|v| (1..=64).contains(&v.len()))
        .ok_or("Expected 1–64 hypothesis outcomes")?;
    let mut ids = BTreeSet::new();
    for outcome in outcomes {
        if !ids.insert(text(&outcome["id"], 50)?) {
            return Err("Repeated outcome identity".into());
        }
        let hypothesis = outcome["hypothesis_id"]
            .as_str()
            .ok_or("Missing hypothesis identity")?;
        if !hypotheses.contains(hypothesis) {
            return Err("Outcome references an absent hypothesis".into());
        }
        text(&outcome["title"], 100)?;
        text(&outcome["definition"], 1000)?;
        text(&outcome["narrative"], 1200)?;
        if let Some(scene) = outcome.get("scene") {
            text(scene, 600)?;
        }
        if let Some(actions) = outcome.get("what_you_can_do") {
            list(actions, 0, 4, 240)?;
        }
        list(&outcome["signals"], 1, 8, 240)?;
        list(&outcome["falsifiers"], 1, 8, 240)?;
        outcome["probability"]
            .as_f64()
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .ok_or("Invalid event probability")?;
        for reference in outcome["scenario_ids"]
            .as_array()
            .ok_or("Missing hypothesis references")?
        {
            if !reference
                .as_str()
                .is_some_and(|id| known_nodes.contains(id))
            {
                return Err("Invented related context reference".into());
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod v2_tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (Value, Value) {
        let outcome = |id: &str, h: &str| json!({"id":id,"hypothesis_id":h,"title":"Future","definition":"Event by the horizon","narrative":"A mechanism and its implications","probability":0.8,"scenario_ids":["h1"],"signals":["Signal"],"falsifiers":["Disconfirmation"]});
        (
            json!({"schema":"foresight-outlook-v2","probability_model":"overlapping_events","probability_basis":"model_implied_event_estimate","calibrated":false,"headline":"Independent events","summary":"Both events may happen","horizon":"2027","evidence_limits":["Limited observations"],"research_questions":[],"outcomes":[outcome("a","h1"),outcome("b","h2")]}),
            json!({"world":{"target_date":"2027"},"nodes":[{"Id":"h1","kind":"scenario"},{"Id":"h2","kind":"revision"},{"Id":"e1","kind":"evidence"}]}),
        )
    }
    #[test]
    fn overlapping_probabilities_are_not_a_partition() {
        let (a, s) = fixture();
        assert!(validate(&a, &s).is_ok());
        let mut a = a;
        a["outcomes"].as_array_mut().unwrap().truncate(1);
        a["outcomes"][0]["scenario_ids"] = json!([]);
        assert!(validate(&a, &s).is_ok());
    }
    #[test]
    fn invented_or_evidence_hypotheses_fail() {
        let (a, s) = fixture();
        for id in ["missing", "e1"] {
            let mut b = a.clone();
            b["outcomes"][0]["hypothesis_id"] = json!(id);
            assert!(validate(&b, &s).is_err());
            let mut b = a.clone();
            b["outcomes"][0]["scenario_ids"] = json!([id]);
            assert_eq!(validate(&b, &s).is_ok(), id == "e1");
        }
    }
    #[test]
    fn event_probability_and_resource_bounds() {
        let (a, s) = fixture();
        for p in [-0.1, 1.1] {
            let mut b = a.clone();
            b["outcomes"][0]["probability"] = json!(p);
            assert!(validate(&b, &s).is_err());
        }
        for (key, n) in [("title", 101), ("definition", 1001), ("narrative", 1201)] {
            let mut b = a.clone();
            b["outcomes"][0][key] = json!("x".repeat(n));
            assert!(validate(&b, &s).is_err());
        }
        let mut b = a.clone();
        b["outcomes"] = json!([]);
        assert!(validate(&b, &s).is_err());
        let mut b = a;
        b["calibrated"] = json!(true);
        assert!(validate(&b, &s).is_err());
    }
    #[test]
    fn optional_scenes_and_actions_preserve_old_answers_and_round_trip() {
        let (mut answer, snapshot) = fixture();
        assert!(
            validate(&answer, &snapshot).is_ok(),
            "old v2 omits new fields"
        );
        answer["outcomes"][0]["scene"] = json!(
            "September 2027: a clinic owner watches her assistant clear the afternoon booking queue."
        );
        answer["outcomes"][0]["what_you_can_do"] =
            json!(["Ask one clinic to show you its last ten failed bookings."]);
        let decoded: Value = serde_json::from_str(&answer.to_string()).unwrap();
        validate(&decoded, &snapshot).unwrap();
        assert_eq!(decoded, answer);
        answer["outcomes"][1]["what_you_can_do"] = json!([]);
        validate(&answer, &snapshot).unwrap();
    }
    #[test]
    fn malformed_scene_and_action_fields_fail_closed() {
        let (answer, snapshot) = fixture();
        for value in [json!(null), json!(42), json!(""), json!("x".repeat(601))] {
            let mut bad = answer.clone();
            bad["outcomes"][0]["scene"] = value;
            assert!(validate(&bad, &snapshot).is_err());
        }
        for value in [
            json!(null),
            json!("a string"),
            json!([42]),
            json!([""]),
            json!(["x".repeat(241)]),
            json!(["a", "b", "c", "d", "e"]),
        ] {
            let mut bad = answer.clone();
            bad["outcomes"][0]["what_you_can_do"] = value;
            assert!(validate(&bad, &snapshot).is_err());
        }
    }
}

/// Present observations remain distinct from assumptions and future hypotheses.
pub fn validate_baseline(baseline: &Value, snapshot: &Value) -> Result<(), String> {
    if baseline["as_of"] != snapshot["world"]["last_ingest_date"] {
        return Err("Baseline vantage differs from the recorded research date".into());
    }
    text(&baseline["as_of"], 32)?;
    list(&baseline["assumptions"], 0, 16, 240)?;
    list(&baseline["unknowns"], 0, 16, 240)?;
    let observed = baseline["observed"]
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or("Invalid baseline observations")?;
    if observed.is_empty()
        && baseline["assumptions"].as_array().is_none_or(Vec::is_empty)
        && baseline["unknowns"].as_array().is_none_or(Vec::is_empty)
    {
        return Err("Missing present baseline or its limitations".into());
    }
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    for observation in observed {
        text(&observation["claim"], 400)?;
        let refs = observation["evidence_ids"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 16)
            .ok_or("Present claim needs evidence references")?;
        for id in refs {
            let node = nodes
                .iter()
                .find(|n| n["Id"] == *id)
                .ok_or("Unknown baseline evidence")?;
            let metadata = &node["evidence_metadata"];
            if !metadata.is_null() && metadata["kind"] != "legacy_unverified" {
                evidence_contract::validate(metadata)?;
                evidence_contract::within_vantage(
                    metadata,
                    baseline["as_of"].as_str().unwrap_or(""),
                )?;
                if metadata["kind"] == "lead" {
                    return Err("Research lead cannot establish a baseline observation".into());
                }
            }
            if snapshot["world"]["evidence_contract"] == "v1" && metadata["kind"] != "finding" {
                return Err("Baseline observation needs a typed finding; legacy evidence belongs in explicit unknowns until verified".into());
            }
            if let (Some(observed), Some(vantage)) =
                (node["observed_at"].as_str(), baseline["as_of"].as_str())
                && observed.len() == 10
                && vantage.len() == 10
                && observed > vantage
            {
                return Err("Baseline evidence is later than its vantage date".into());
            }
            if matches!(
                node["kind"].as_str(),
                Some("scenario" | "revision" | "world" | "hypothesis" | "option")
            ) {
                return Err("A future hypothesis cannot establish the present".into());
            }
        }
    }
    Ok(())
}

fn validate_v3(answer: &Value, snapshot: &Value) -> Result<(), String> {
    if answer["probability_basis"] != "model_implied_world_estimate"
        || answer["probability_model"] != "overlapping_worlds"
        || answer["calibrated"] != false
    {
        return Err(
            "World odds must identify overlapping uncalibrated whole-world estimates".into(),
        );
    }
    text(&answer["headline"], 160)?;
    text(&answer["summary"], 400)?;
    if answer["horizon"] != snapshot["world"]["target_date"] {
        return Err("World horizon differs from question".into());
    }
    validate_baseline(&answer["baseline"], snapshot)?;
    list(&answer["evidence_limits"], 1, 32, 240)?;
    list(&answer["research_questions"], 0, 64, 240)?;
    answer["evaluation_note"]
        .as_str()
        .filter(|v| v.chars().count() <= 2000)
        .ok_or("Invalid evaluation note")?;
    let nodes = snapshot["nodes"].as_array().ok_or("Missing nodes")?;
    let worlds: std::collections::BTreeMap<_, _> = nodes
        .iter()
        .filter(|n| n["kind"] == "world" && n["archived"] != true)
        .filter_map(|n| n["Id"].as_str().map(|id| (id, n)))
        .collect();
    let outcomes = answer["outcomes"]
        .as_array()
        .filter(|v| (2..=6).contains(&v.len()))
        .ok_or("Expected 2–6 composed worlds")?;
    let mut seen = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut evaluated = 0;
    for outcome in outcomes {
        if !ids.insert(text(&outcome["id"], 50)?) {
            return Err("Repeated outcome identity".into());
        }
        let id = outcome["world_id"]
            .as_str()
            .ok_or("Missing world identity")?;
        let world = worlds
            .get(id)
            .ok_or("Outcome must reference a composed world, not an individual event")?;
        if !seen.insert(id) {
            return Err("Repeated composed world".into());
        }
        for (key, limit) in [
            ("title", 100),
            ("definition", 1000),
            ("scene", 600),
            ("narrative", 1200),
        ] {
            text(&outcome[key], limit)?;
        }
        if outcome["definition"] != world["statement"]
            || outcome["component_ids"] != world["component_ids"]
            || outcome["counter_ids"] != world["counter_ids"]
        {
            return Err("World meaning or defining links changed after evaluation".into());
        }
        let components = outcome["component_ids"]
            .as_array()
            .filter(|v| (2..=12).contains(&v.len()))
            .ok_or("A world needs multiple defining events")?;
        if components
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            != components.len()
        {
            return Err("Duplicate world components".into());
        }
        for component in components {
            if !nodes.iter().any(|n| {
                n["Id"] == *component && matches!(n["kind"].as_str(), Some("scenario" | "revision"))
            }) {
                return Err("World component is not an explored hypothesis".into());
            }
        }
        let counters = outcome["counter_ids"]
            .as_array()
            .filter(|v| v.len() <= 12)
            .ok_or("Invalid world challenges")?;
        for counter in counters {
            if components.contains(counter)
                || !nodes.iter().any(|n| {
                    n["Id"] == *counter
                        && matches!(n["kind"].as_str(), Some("scenario" | "revision"))
                })
            {
                return Err("Invalid world counter-hypothesis".into());
            }
        }
        list(&outcome["what_you_can_do"], 0, 4, 240)?;
        list(&outcome["signals"], 1, 8, 240)?;
        list(&outcome["falsifiers"], 1, 8, 240)?;
        for reference in outcome["scenario_ids"]
            .as_array()
            .ok_or("Missing world context")?
        {
            if !nodes.iter().any(|n| n["Id"] == *reference) {
                return Err("Invented world context reference".into());
            }
        }
        if !outcome["probability"].is_null() {
            outcome["probability"]
                .as_f64()
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .ok_or("Invalid whole-world probability")?;
            evaluated += 1;
        }
    }
    if seen.len() != worlds.len() {
        return Err("Answer omitted a composed world".into());
    }
    let expected = if evaluated == 0 {
        "unavailable"
    } else if evaluated == outcomes.len() {
        "evaluated"
    } else {
        "partial"
    };
    if answer["evaluation_status"] != expected {
        return Err("World evaluation status overstates available odds".into());
    }
    Ok(())
}

#[cfg(test)]
mod world_tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (Value, Value) {
        let w = |id: &str| json!({"Id":id,"kind":"world","statement":format!("Joint world {id} by 2027"),"component_ids":["a","b"],"counter_ids":[]});
        let snapshot = json!({"world":{"last_ingest_date":"2026-09-19","target_date":"2027"},"nodes":[{"Id":"e","kind":"evidence"},{"Id":"a","kind":"scenario"},{"Id":"b","kind":"revision"},w("w1"),w("w2")]});
        let o = |id: &str| json!({"id":id,"world_id":id,"title":"A world you can picture","definition":format!("Joint world {id} by 2027"),"component_ids":["a","b"],"counter_ids":[],"scenario_ids":["e"],"scene":"Imagine your day changing.","narrative":"Why these changes happen together.","what_you_can_do":[],"signals":["Watch this"],"falsifiers":["This would undermine it"],"probability":0.23});
        let answer = json!({"schema":"foresight-worlds-v3","headline":"Different worlds","summary":"What changes after today","horizon":"2027","probability_basis":"model_implied_world_estimate","probability_model":"overlapping_worlds","calibrated":false,"evaluation_status":"evaluated","evaluation_note":"","baseline":{"as_of":"2026-09-19","observed":[{"claim":"Already happening","evidence_ids":["e"]}],"assumptions":[],"unknowns":[]},"evidence_limits":["Limited research"],"research_questions":[],"outcomes":[o("w1"),o("w2")]});
        (answer, snapshot)
    }
    #[test]
    fn whole_worlds_accept_independent_odds_and_explicit_missing_evaluations() {
        let (mut a, s) = fixture();
        assert!(validate(&a, &s).is_ok());
        a["outcomes"][0]["probability"] = Value::Null;
        assert!(validate(&a, &s).is_err());
        a["evaluation_status"] = json!("partial");
        assert!(validate(&a, &s).is_ok());
        a["outcomes"][1]["probability"] = Value::Null;
        a["evaluation_status"] = json!("unavailable");
        assert!(validate(&a, &s).is_ok());
    }
    #[test]
    fn component_cannot_masquerade_as_world_or_change_world_meaning() {
        let (a, s) = fixture();
        for (key, v) in [
            ("world_id", json!("a")),
            ("definition", json!("An easier event")),
            ("component_ids", json!(["a"])),
        ] {
            let mut bad = a.clone();
            bad["outcomes"][0][key] = v;
            assert!(validate(&bad, &s).is_err());
        }
    }
    #[test]
    fn hypothetical_future_is_not_observed_present_and_vantage_is_exact() {
        let (a, s) = fixture();
        let mut bad = a.clone();
        bad["baseline"]["observed"][0]["evidence_ids"] = json!(["a"]);
        assert!(validate(&bad, &s).is_err());
        let mut future = s.clone();
        future["nodes"][0]["observed_at"] = json!("2027-01-01");
        assert!(validate(&a, &future).is_err());
        let mut bad = a;
        bad["baseline"]["as_of"] = json!("2027");
        assert!(validate(&bad, &s).is_err());
    }
}
