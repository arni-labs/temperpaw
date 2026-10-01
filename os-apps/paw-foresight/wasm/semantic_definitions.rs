// One vocabulary for Jev evaluations and the reasoning context that consumes them.
use serde_json::{Value, json};
pub fn evaluate_novelty() -> Value {
    json!([
        "Duplicates an existing hypothesis or adds no distinct mechanism.",
        "Minor variation in an existing mechanism.",
        "Distinct plausible mechanism or interaction worth exploring.",
        "Substantially new mechanism revealing an overlooked path.",
        "Strongly differentiated mechanism that changes how the question should be investigated."
    ])
}
pub fn decision_value() -> Value {
    json!([
        "No identifiable decision or information value.",
        "Limited consequences or weak discriminating information.",
        "Useful information for a concrete decision.",
        "High consequence and a useful way to discriminate outcomes.",
        "Critical decision relevance with strong value from resolving uncertainty."
    ])
}
pub fn choose_next_operation() -> Value {
    json!({"brainstorm":"Generate materially different hypotheses or mechanisms.","research":"Seek new evidence to resolve an unsupported premise.","challenge":"Look for contrary evidence or a falsifying mechanism.","connect":"Investigate an interaction between existing mechanisms.","deepen":"Develop a causal mechanism or its implications in greater detail.","repair":"Correct a specific inadequate mechanism or timing.","monitor":"Retain the hypothesis and monitor dated discriminating signals.","uncertain":"Available information cannot identify a useful next operation."})
}
