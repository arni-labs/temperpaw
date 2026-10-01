//! Durable mid-snippet progress uses Session.CheckpointToolBatch, never a
//! self-POST. Only completed external work can reach this boundary.
use super::*;

pub const FORMAT: &str = "monty-inner-call-v1";
// A fetched page can spend about 120s in host I/O. Yield after that completed
// operation, leaving 180s of the Session's 300s watchdog for persistence. A
// single stuck operation never reaches this boundary and still times out.
const CHECKPOINT_AFTER_MS: i64 = 120_000;

pub fn elapsed_due(elapsed_ms: i64) -> bool {
    elapsed_ms >= CHECKPOINT_AFTER_MS
}

pub fn checkpoint_due(progress: &ReplProgress<LimitedTracker>, elapsed_ms: i64) -> bool {
    elapsed_due(elapsed_ms)
        && matches!(
            progress,
            ReplProgress::FunctionCall(_) | ReplProgress::Complete { .. }
        )
}

// Both checkpoint paths continue the still-pending call on persistence failure.
pub fn saved_checkpoint_or_continue(
    saved: Result<String, String>,
    on_error: impl FnOnce(&str),
) -> Option<String> {
    match saved {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) => {
            on_error("checkpoint needs a persisted workspace file");
            None
        }
        Err(error) => {
            on_error(&error);
            None
        }
    }
}

pub struct Saved {
    pub progress: ReplProgress<LimitedTracker>,
    pub printed: BoundedOutputCollector,
    pub tool_events: Vec<Value>,
    pub tool_started_ms: i64,
    pub dispatch_state: Value,
}

pub fn encode(
    progress: &ReplProgress<LimitedTracker>,
    printed: &BoundedOutputCollector,
    tool_id: &str,
    tool_started_ms: i64,
    tool_events: &[Value],
) -> Result<String, String> {
    let progress = progress
        .dump()
        .map_err(|e| format!("cannot serialize in-flight REPL: {e}"))?;
    Ok(json!({"format":FORMAT,"progress":base64_encode(&progress),
        "tool_id":tool_id,"tool_started_ms":tool_started_ms,"tool_events":tool_events,
        "printed":{"buf":printed.buf,"max_bytes":printed.max_bytes,
            "total_bytes_seen":printed.total_bytes_seen,"truncated":printed.truncated},
        "dispatch":dispatch::continuation_state()})
    .to_string())
}

impl Saved {
    pub fn decode(raw: &str, current_tool: Option<&Value>) -> Result<Self, String> {
        let value: Value =
            serde_json::from_str(raw).map_err(|e| format!("invalid continuation file: {e}"))?;
        if value["format"] != FORMAT
            || current_tool.is_none()
            || value["tool_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .is_none()
            || value["tool_id"] != current_tool.unwrap()["id"]
        {
            return Err("continuation does not match the pending tool".into());
        }
        let progress = value["progress"].as_str().ok_or("missing in-flight REPL")?;
        let progress = ReplProgress::load(&base64_decode(progress)?)
            .map_err(|e| format!("invalid in-flight REPL: {e}"))?;
        let data = &value["printed"];
        let buf = data["buf"].as_str().ok_or("missing checkpoint output")?;
        let total_bytes_seen = data["total_bytes_seen"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("invalid checkpoint output length")?;
        if data["max_bytes"] != MAX_TOOL_RESULT_BYTES
            || buf.len() > MAX_TOOL_RESULT_BYTES
            || total_bytes_seen < buf.len()
        {
            return Err("invalid bounded checkpoint output".into());
        }
        Ok(Self {
            progress,
            printed: BoundedOutputCollector {
                buf: buf.into(),
                max_bytes: MAX_TOOL_RESULT_BYTES,
                total_bytes_seen,
                truncated: data["truncated"]
                    .as_bool()
                    .ok_or("invalid output truncation flag")?,
            },
            tool_events: value["tool_events"]
                .as_array()
                .ok_or("missing checkpoint tool events")?
                .clone(),
            tool_started_ms: value["tool_started_ms"]
                .as_i64()
                .ok_or("missing outer tool start time")?,
            dispatch_state: value["dispatch"].clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(code: &str, printed: &mut BoundedOutputCollector) -> ReplProgress<LimitedTracker> {
        MontyRepl::new(
            "continuation-test.py",
            LimitedTracker::new(ResourceLimits::new()),
        )
        .feed_start(code, vec![], PrintWriter::Callback(printed))
        .unwrap()
    }

    // Deterministic component simulation: real pinned Monty continuation and
    // production checkpoint codec/predicate, virtual external durations/storage.
    // Native Session watchdog/action behavior is verified separately on deploy.
    fn simulate(durations: &[i64], fail_saves: usize) -> (Vec<usize>, String, usize) {
        let mut printed = BoundedOutputCollector::new(MAX_TOOL_RESULT_BYTES);
        let code = format!(
            "values = []\nfor i in range({}):\n    print('before', i)\n    values.append(fetch(i))\n    print('after', i)\nvalues",
            durations.len()
        );
        let mut progress = start(&code, &mut printed);
        let mut calls = Vec::new();
        let mut events = Vec::new();
        let mut elapsed = 0;
        let mut checkpoints = 0;
        let mut attempts = 0;
        loop {
            if checkpoint_due(&progress, elapsed) {
                attempts += 1;
                if attempts > fail_saves {
                    let saved = encode(&progress, &printed, "outer", 42, &events).unwrap();
                    let restored = Saved::decode(&saved, Some(&json!({"id":"outer"}))).unwrap();
                    assert_eq!(restored.tool_started_ms, 42);
                    assert_eq!(restored.tool_events, events);
                    progress = restored.progress;
                    printed = restored.printed;
                    checkpoints += 1;
                    elapsed = 0;
                }
                // A failed save leaves the exact live interpreter untouched.
            }
            match progress {
                ReplProgress::FunctionCall(call) => {
                    let index = calls.len();
                    assert_eq!(call.args, vec![MontyObject::Int(index as i64)]);
                    calls.push(index);
                    elapsed += durations[index];
                    assert!(
                        elapsed < 300_000,
                        "Session watchdog would expire at {elapsed}ms"
                    );
                    events.push(json!({"tool":index}));
                    progress = call
                        .resume(
                            MontyObject::Int(index as i64),
                            PrintWriter::Callback(&mut printed),
                        )
                        .unwrap();
                }
                ReplProgress::Complete { value, .. } => {
                    assert_eq!(
                        value,
                        MontyObject::List(
                            (0..durations.len())
                                .map(|i| MontyObject::Int(i as i64))
                                .collect()
                        )
                    );
                    return (calls, printed.into_string(), checkpoints);
                }
                _ => panic!("unexpected interpreter suspension"),
            }
        }
    }

    #[test]
    fn absent_workspace_cannot_publish_an_empty_checkpoint_pointer() {
        assert!(saved_checkpoint_or_continue(Ok(String::new()), |_| {}).is_none());
        assert_eq!(
            saved_checkpoint_or_continue(Ok("file".into()), |_| {}),
            Some("file".into())
        );
    }

    #[test]
    fn failed_outer_checkpoint_leaves_current_call_to_execute() {
        let mut executed = Vec::new();
        let mut elapsed = 0;
        for index in 0..6 {
            if index > 0
                && elapsed_due(elapsed)
                && let Some(_) =
                    saved_checkpoint_or_continue(Err("disk unavailable".into()), |_| {})
            {
                panic!("failed save must not publish checkpoint");
            }
            // Same fall-through as run_tools: cursor advances only after execution.
            executed.push(index);
            elapsed += 50_000;
        }
        assert_eq!(executed, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn six_slow_calls_checkpoint_without_replay_or_output_loss() {
        let (calls, output, count) = simulate(&[121_000; 6], 0);
        assert_eq!(calls, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(
            output,
            (0..6)
                .map(|i| format!("before {i}\nafter {i}\n"))
                .collect::<String>()
        );
        assert_eq!(count, 6); // Includes Complete: slow final call checkpoints safely too.
    }

    #[test]
    fn failed_save_keeps_live_progress_and_next_checkpoint_retries_it() {
        let (calls, output, count) = simulate(&[121_000; 6], 1);
        assert_eq!(calls, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(output.matches("after").count(), 6);
        assert_eq!(count, 5);
    }

    #[test]
    fn cheap_calls_do_not_checkpoint_and_hung_call_cannot_signal_progress() {
        assert_eq!(simulate(&[2, 3], 0).2, 0);
        assert!(!elapsed_due(119_999));
        assert!(elapsed_due(120_000));
        // Before dispatch returns there is no completed result to serialize.
        assert!(std::panic::catch_unwind(|| simulate(&[301_000], 0)).is_err());
    }

    #[test]
    fn continuation_preserves_bounded_print_and_rejects_wrong_identity_or_corruption() {
        let mut printed = BoundedOutputCollector::new(MAX_TOOL_RESULT_BYTES);
        let progress = start("fetch(1)", &mut printed);
        printed.append_str(&"é".repeat(MAX_TOOL_RESULT_BYTES));
        let saved = encode(&progress, &printed, "outer", 0, &[]).unwrap();
        assert!(Saved::decode(&saved, Some(&json!({"id":"other"}))).is_err());
        assert!(Saved::decode("", Some(&json!({"id":"outer"}))).is_err());
        assert!(Saved::decode(&saved, None).is_err());
        let restored = Saved::decode(&saved, Some(&json!({"id":"outer"}))).unwrap();
        assert_eq!(restored.printed.buf, printed.buf);
        assert_eq!(restored.printed.total_bytes_seen, printed.total_bytes_seen);
        assert!(restored.printed.truncated);
    }

    #[test]
    fn exception_after_checkpoint_keeps_earlier_side_effect_and_prints() {
        let mut printed = BoundedOutputCollector::new(MAX_TOOL_RESULT_BYTES);
        let progress = start(
            "x=fetch(0)\nprint(x)\nfetch(1)\nraise ValueError('after calls')",
            &mut printed,
        );
        let call = progress.into_function_call().unwrap();
        let progress = call
            .resume(MontyObject::Int(7), PrintWriter::Callback(&mut printed))
            .unwrap();
        let saved = encode(&progress, &printed, "outer", 0, &[]).unwrap();
        let restored = Saved::decode(&saved, Some(&json!({"id":"outer"}))).unwrap();
        printed = restored.printed;
        let call = restored.progress.into_function_call().unwrap();
        assert_eq!(call.args, vec![MontyObject::Int(1)]);
        let error = call
            .resume(MontyObject::None, PrintWriter::Callback(&mut printed))
            .unwrap_err();
        assert!(format_monty_exception(&error.error).contains("after calls"));
        assert_eq!(printed.into_string(), "7\n");
    }
}
