// turn_runner does one thing: it sends one model turn to the session's provider and answers what came back. The
// history it is handed is provider-neutral; this module writes it in the provider's own wire format:
//   openai, openrouter  Chat Completions (`chat/completions`), the key in Authorization (codex_api_key, openrouter_api_key)
//   anthropic           Messages (`messages`), the key in x-api-key (anthropic_api_key)
// Each key is put in place by its door, out of this module's sight. The spec, not this module, turns the answer into a
// move: stop "tool_use" -> answered_tools, "end" -> answered or compacted, anything else (an error status, an unreadable
// reply) -> turn_refused with the reason in `stop` and the start of the provider's body in `text`.
//
// Two tools, each offered only when the agent holds it: `bash`, run on the session's computer, and `unikon`, one UniKon
// connector call made by unikon_door with no computer. Limits, each with its
// reason: one door call per turn, since one turn is one request; max_tokens is the agent's, at least 1 as providers ask;
// an error body is kept to 600 bytes, enough to say why and small beside the session's row.

const BASH_DESCRIPTION: &'static str = "Run one bash command on your own computer (a persistent Linux sandbox with your \
workspace in /home/tl-user). It answers the exit code, stdout and stderr. Each command runs at most 120 seconds.";
const UNIKON_DESCRIPTION: &'static str = "Call one UniKon connector tool directly, as your owner: tool is its name (such as \
find_app, inspect_app, use_app, wait_for_app, evolve_app, set_app_interface, construct_app, get_upload_urls, \
construction_manual), arguments its JSON arguments, exactly as the connector takes them. It answers the tool's text. \
Send tool list_tools for every tool and what it does.";
const COMPACT_ASK: &'static str = "Summarize this conversation so far for yourself: the task, what was done, what was \
learned, and what is left. This summary will replace the conversation.";
const ERROR_KEEP: usize = 600;

// Every answer carries the epoch (the run) it was asked in, so the spec moves nothing on an answer to a run a stop ended.
pub fn call(input: Input) -> Option<Answer> {
    let epoch = input.epoch;
    match reply_to(input) {
        Some(mut answer) => {
            answer.epoch = epoch;
            Some(answer)
        }
        None => None,
    }
}

fn reply_to(input: Input) -> Option<Answer> {
    let anthropic = input.provider.as_str() == "anthropic";
    let openrouter = input.provider.as_str() == "openrouter";
    let body = if anthropic { anthropic_body(&input) } else { chat_body(&input, openrouter) };
    let mut headers: Vec<(String, String)> = Vec::new();
    headers.push(("content-type".to_string(), "application/json".to_string()));
    let reply = if anthropic {
        headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
        door_call(DoorRequest {
            destination: DoorDestination::anthropic, method: DoorMethod::Post, path: "messages".to_string(), headers, body: body.into_bytes(),
        })
    } else if openrouter {
        door_call(DoorRequest {
            destination: DoorDestination::openrouter, method: DoorMethod::Post, path: "chat/completions".to_string(), headers, body: body.into_bytes(),
        })
    } else {
        door_call(DoorRequest {
            destination: DoorDestination::openai, method: DoorMethod::Post, path: "chat/completions".to_string(), headers, body: body.into_bytes(),
        })
    };
    match reply {
        DoorReply::Answered(heard) => {
            let DoorAnswer { status, headers: _, body: raw } = heard;
            let body = text_of(raw);
            if status < 200 || status > 299 {
                return Some(failed_answer(status, &body));
            }
            let read = if anthropic { anthropic_reply(&body) } else { chat_reply(&body) };
            match read {
                Some(answer) => Some(answer),
                None => Some(failed_answer(0, &body)),
            }
        }
        // Failed, refused and unknown end the run as such; the spec's failed path takes them.
        _ => None,
    }
}

fn empty_answer() -> Answer {
    Answer {
        message: String::new(), stop: String::new(), text: String::new(), call_ids: Vec::new(), call_names: Vec::new(),
        call_args: Vec::new(), tokens_in: 0, tokens_out: 0, epoch: 0,
    }
}

// status 0 is a 2xx reply this module could not read.
fn failed_answer(status: u64, body: &str) -> Answer {
    let mut answer = empty_answer();
    if status == 0 {
        answer.stop.push_str("unreadable");
    } else {
        let m = if status > 999 { 999 } else { status };
        answer.stop.push_str("http_");
        answer.stop.push_str(digit_text(m / 100));
        answer.stop.push_str(digit_text((m / 10) % 10));
        answer.stop.push_str(digit_text(m % 10));
    }
    answer.text = cut_at_ascii(body, ERROR_KEEP);
    answer
}

// The text up to `keep` bytes, cut back to the last ASCII byte so the cut falls between characters.
fn cut_at_ascii(s: &str, keep: usize) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    if n <= keep {
        return s.to_string();
    }
    let mut end: usize = keep;
    while end > 0 && bytes[end] >= 128
        invariant n == bytes.len(), end <= keep, keep < n,
        decreases end,
    {
        end = end - 1;
    }
    match s.get(0..end) { Some(t) => t.to_string(), None => String::new() }
}

fn bounded_tokens(n: i64) -> u64 {
    if n < 1 { 1 } else if n > 999_999 { 999_999 } else { n as u64 }
}

// A tool call's input as JSON text: what the model wrote, or an empty object.
fn args_or_empty(args: &str) -> &str {
    if args.trim().len() == 0 { "{}" } else { args }
}

// ---- Chat Completions (openai, openrouter)

fn chat_body(input: &Input, openrouter: bool) -> String {
    let mut b = String::new();
    b.push_str("{\"model\":");
    push_json_string(&mut b, &input.model);
    // OpenAI's newer models take max_completion_tokens and refuse max_tokens; OpenRouter takes max_tokens.
    if openrouter { b.push_str(",\"max_tokens\":"); } else { b.push_str(",\"max_completion_tokens\":"); }
    push_count(&mut b, bounded_tokens(input.max_tokens));
    if (input.computer || input.unikon) && !input.compact {
        b.push_str(",\"tools\":[");
        if input.computer {
            b.push_str("{\"type\":\"function\",\"function\":{\"name\":\"bash\",\"description\":");
            push_json_string(&mut b, BASH_DESCRIPTION);
            b.push_str(",\"parameters\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}},\"required\":[\"command\"]}}}");
        }
        if input.unikon {
            if input.computer {
                b.push_str(",");
            }
            b.push_str("{\"type\":\"function\",\"function\":{\"name\":\"unikon\",\"description\":");
            push_json_string(&mut b, UNIKON_DESCRIPTION);
            b.push_str(",\"parameters\":{\"type\":\"object\",\"properties\":{\"tool\":{\"type\":\"string\"},\"arguments\":{\"type\":\"object\"}},\"required\":[\"tool\"]}}}");
        }
        b.push_str("]");
    }
    b.push_str(",\"messages\":[");
    let mut first = true;
    if input.soul.len() > 0 {
        b.push_str("{\"role\":\"system\",\"content\":");
        push_json_string(&mut b, &input.soul);
        b.push_str("}");
        first = false;
    }
    let count = input.history.len();
    let mut j: usize = 0;
    while j < count
        invariant count == input.history.len(),
        decreases count - j,
    {
        if !first {
            b.push_str(",");
        }
        push_chat_entry(&mut b, &input.history[j]);
        first = false;
        j = j + 1;
    }
    if input.compact {
        if !first {
            b.push_str(",");
        }
        b.push_str("{\"role\":\"user\",\"content\":");
        push_json_string(&mut b, COMPACT_ASK);
        b.push_str("}");
    }
    b.push_str("]}");
    b
}

fn push_chat_entry(b: &mut String, e: &Input_history_item) {
    let role = e.role.as_str();
    if role == "tool" {
        b.push_str("{\"role\":\"tool\",\"tool_call_id\":");
        push_json_string(b, &e.call_id);
        b.push_str(",\"content\":");
        push_json_string(b, &e.content);
        b.push_str("}");
    } else if role == "assistant" {
        b.push_str("{\"role\":\"assistant\",\"content\":");
        push_json_string(b, &e.content);
        let calls = e.call_ids.len();
        if calls > 0 && e.call_names.len() == calls && e.call_args.len() == calls {
            b.push_str(",\"tool_calls\":[");
            let mut k: usize = 0;
            while k < calls
                invariant calls == e.call_ids.len(), calls == e.call_names.len(), calls == e.call_args.len(),
                decreases calls - k,
            {
                if k > 0 {
                    b.push_str(",");
                }
                b.push_str("{\"id\":");
                push_json_string(b, &e.call_ids[k]);
                b.push_str(",\"type\":\"function\",\"function\":{\"name\":");
                push_json_string(b, &e.call_names[k]);
                b.push_str(",\"arguments\":");
                push_json_string(b, args_or_empty(&e.call_args[k]));
                b.push_str("}}");
                k = k + 1;
            }
            b.push_str("]");
        }
        b.push_str("}");
    } else {
        b.push_str("{\"role\":\"user\",\"content\":");
        push_json_string(b, &e.content);
        b.push_str("}");
    }
}

fn chat_reply(body: &str) -> Option<Answer> {
    let bytes = body.as_bytes();
    let top = match object_fields(bytes, skip_ws(bytes, 0)) { Some(f) => f, None => return None };
    let mut answer = empty_answer();
    let mut finish = String::new();
    let count = top.len();
    let mut j: usize = 0;
    while j < count
        invariant count == top.len(),
        decreases count - j,
    {
        let key = top[j].0.as_str();
        let at = top[j].1;
        if key == "id" {
            match read_string(bytes, at) { Some((s, _)) => answer.message = s, None => {} }
        } else if key == "usage" {
            match object_fields(bytes, at) {
                Some(usage) => {
                    let uses = usage.len();
                    let mut u: usize = 0;
                    while u < uses
                        invariant uses == usage.len(),
                        decreases uses - u,
                    {
                        let name = usage[u].0.as_str();
                        if name == "prompt_tokens" {
                            answer.tokens_in = read_count(bytes, usage[u].1) as i64;
                        } else if name == "completion_tokens" {
                            answer.tokens_out = read_count(bytes, usage[u].1) as i64;
                        }
                        u = u + 1;
                    }
                }
                None => {}
            }
        } else if key == "choices" {
            let choices = match array_items(bytes, at) { Some(items) => items, None => return None };
            if choices.len() > 0 {
                match read_choice(bytes, choices[0].0, &mut answer) { Some(f) => finish = f, None => return None }
            }
        }
        j = j + 1;
    }
    let f = finish.as_str();
    if answer.call_ids.len() > 0 && (f == "tool_calls" || f == "stop") {
        answer.stop.push_str("tool_use");
    } else if f == "stop" || f == "length" {
        answer.stop.push_str("end");
    } else {
        answer.stop.push_str("finish_");
        answer.stop.push_str(f);
    }
    Some(answer)
}

// One choice: its message's text and tool calls go into the answer; it answers the finish reason.
fn read_choice(bytes: &[u8], at: usize, answer: &mut Answer) -> Option<String> {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return None };
    let mut finish = String::new();
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        let vat = fields[j].1;
        if key == "finish_reason" {
            match read_string(bytes, vat) { Some((s, _)) => finish = s, None => {} }
        } else if key == "message" {
            if !read_chat_message(bytes, vat, answer) {
                return None;
            }
        }
        j = j + 1;
    }
    Some(finish)
}

fn read_chat_message(bytes: &[u8], at: usize, answer: &mut Answer) -> bool {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return false };
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        let vat = fields[j].1;
        if key == "content" {
            // null when the message only calls tools.
            match read_string(bytes, vat) { Some((s, _)) => answer.text = s, None => {} }
        } else if key == "tool_calls" {
            let calls = match array_items(bytes, vat) { Some(items) => items, None => return false };
            let many = calls.len();
            let mut k: usize = 0;
            while k < many
                invariant many == calls.len(),
                decreases many - k,
            {
                if !read_chat_call(bytes, calls[k].0, answer) {
                    return false;
                }
                k = k + 1;
            }
        }
        j = j + 1;
    }
    true
}

// One tool call: {"id", "type", "function": {"name", "arguments": "<JSON text>"}}.
fn read_chat_call(bytes: &[u8], at: usize, answer: &mut Answer) -> bool {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return false };
    let mut id = String::new();
    let mut name = String::new();
    let mut args = String::new();
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        let vat = fields[j].1;
        if key == "id" {
            match read_string(bytes, vat) { Some((s, _)) => id = s, None => return false }
        } else if key == "function" {
            let inner = match object_fields(bytes, vat) { Some(f) => f, None => return false };
            let parts = inner.len();
            let mut p: usize = 0;
            while p < parts
                invariant parts == inner.len(),
                decreases parts - p,
            {
                let part = inner[p].0.as_str();
                if part == "name" {
                    match read_string(bytes, inner[p].1) { Some((s, _)) => name = s, None => return false }
                } else if part == "arguments" {
                    match read_string(bytes, inner[p].1) { Some((s, _)) => args = s, None => return false }
                }
                p = p + 1;
            }
        }
        j = j + 1;
    }
    if id.len() == 0 || name.len() == 0 {
        return false;
    }
    answer.call_ids.push(id);
    answer.call_names.push(name);
    answer.call_args.push(args);
    true
}

// ---- Messages (anthropic)

fn anthropic_body(input: &Input) -> String {
    let mut b = String::new();
    b.push_str("{\"model\":");
    push_json_string(&mut b, &input.model);
    b.push_str(",\"max_tokens\":");
    push_count(&mut b, bounded_tokens(input.max_tokens));
    if input.soul.len() > 0 {
        b.push_str(",\"system\":");
        push_json_string(&mut b, &input.soul);
    }
    if (input.computer || input.unikon) && !input.compact {
        b.push_str(",\"tools\":[");
        if input.computer {
            b.push_str("{\"name\":\"bash\",\"description\":");
            push_json_string(&mut b, BASH_DESCRIPTION);
            b.push_str(",\"input_schema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}},\"required\":[\"command\"]}}");
        }
        if input.unikon {
            if input.computer {
                b.push_str(",");
            }
            b.push_str("{\"name\":\"unikon\",\"description\":");
            push_json_string(&mut b, UNIKON_DESCRIPTION);
            b.push_str(",\"input_schema\":{\"type\":\"object\",\"properties\":{\"tool\":{\"type\":\"string\"},\"arguments\":{\"type\":\"object\"}},\"required\":[\"tool\"]}}");
        }
        b.push_str("]");
    }
    // Anthropic joins consecutive turns of one role into one, so each entry may be its own message.
    b.push_str(",\"messages\":[");
    let mut first = true;
    let count = input.history.len();
    let mut j: usize = 0;
    while j < count
        invariant count == input.history.len(),
        decreases count - j,
    {
        if !first {
            b.push_str(",");
        }
        push_anthropic_entry(&mut b, &input.history[j]);
        first = false;
        j = j + 1;
    }
    if input.compact || first {
        if !first {
            b.push_str(",");
        }
        b.push_str("{\"role\":\"user\",\"content\":");
        push_json_string(&mut b, COMPACT_ASK);
        b.push_str("}");
    }
    b.push_str("]}");
    b
}

fn push_anthropic_entry(b: &mut String, e: &Input_history_item) {
    let role = e.role.as_str();
    if role == "tool" {
        b.push_str("{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":");
        push_json_string(b, &e.call_id);
        b.push_str(",\"content\":");
        push_json_string(b, &e.content);
        b.push_str("}]}");
    } else if role == "assistant" {
        b.push_str("{\"role\":\"assistant\",\"content\":[");
        let calls = e.call_ids.len();
        let has_calls = calls > 0 && e.call_names.len() == calls && e.call_args.len() == calls;
        let mut wrote = false;
        if e.content.len() > 0 || !has_calls {
            b.push_str("{\"type\":\"text\",\"text\":");
            if e.content.len() > 0 { push_json_string(b, &e.content); } else { b.push_str("\"(no text)\""); }
            b.push_str("}");
            wrote = true;
        }
        if has_calls {
            let mut k: usize = 0;
            while k < calls
                invariant calls == e.call_ids.len(), calls == e.call_names.len(), calls == e.call_args.len(),
                decreases calls - k,
            {
                if wrote {
                    b.push_str(",");
                }
                b.push_str("{\"type\":\"tool_use\",\"id\":");
                push_json_string(b, &e.call_ids[k]);
                b.push_str(",\"name\":");
                push_json_string(b, &e.call_names[k]);
                b.push_str(",\"input\":");
                b.push_str(args_or_empty(&e.call_args[k]));
                b.push_str("}");
                wrote = true;
                k = k + 1;
            }
        }
        b.push_str("]}");
    } else {
        b.push_str("{\"role\":\"user\",\"content\":");
        push_json_string(b, &e.content);
        b.push_str("}");
    }
}

fn anthropic_reply(body: &str) -> Option<Answer> {
    let bytes = body.as_bytes();
    let top = match object_fields(bytes, skip_ws(bytes, 0)) { Some(f) => f, None => return None };
    let mut answer = empty_answer();
    let mut reason = String::new();
    let count = top.len();
    let mut j: usize = 0;
    while j < count
        invariant count == top.len(),
        decreases count - j,
    {
        let key = top[j].0.as_str();
        let at = top[j].1;
        if key == "id" {
            match read_string(bytes, at) { Some((s, _)) => answer.message = s, None => {} }
        } else if key == "stop_reason" {
            match read_string(bytes, at) { Some((s, _)) => reason = s, None => {} }
        } else if key == "usage" {
            match object_fields(bytes, at) {
                Some(usage) => {
                    let uses = usage.len();
                    let mut u: usize = 0;
                    while u < uses
                        invariant uses == usage.len(),
                        decreases uses - u,
                    {
                        let name = usage[u].0.as_str();
                        if name == "input_tokens" {
                            answer.tokens_in = read_count(bytes, usage[u].1) as i64;
                        } else if name == "output_tokens" {
                            answer.tokens_out = read_count(bytes, usage[u].1) as i64;
                        }
                        u = u + 1;
                    }
                }
                None => {}
            }
        } else if key == "content" {
            let blocks = match array_items(bytes, at) { Some(items) => items, None => return None };
            let many = blocks.len();
            let mut k: usize = 0;
            while k < many
                invariant many == blocks.len(),
                decreases many - k,
            {
                if !read_block(body, bytes, blocks[k].0, &mut answer) {
                    return None;
                }
                k = k + 1;
            }
        }
        j = j + 1;
    }
    let r = reason.as_str();
    if r == "tool_use" && answer.call_ids.len() > 0 {
        answer.stop.push_str("tool_use");
    } else if r == "end_turn" || r == "max_tokens" || r == "stop_sequence" {
        answer.stop.push_str("end");
    } else {
        answer.stop.push_str("stop_");
        answer.stop.push_str(r);
    }
    Some(answer)
}

// One content block: text is joined into the answer's text; tool_use gives one call, its input kept as raw JSON.
fn read_block(body: &str, bytes: &[u8], at: usize, answer: &mut Answer) -> bool {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return false };
    let mut kind = String::new();
    let mut text = String::new();
    let mut id = String::new();
    let mut name = String::new();
    let mut args = String::new();
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        if key == "input" {
            args = match body.get(fields[j].1..fields[j].2) { Some(raw) => raw.to_string(), None => return false };
        } else if key == "type" || key == "text" || key == "id" || key == "name" {
            let value = match read_string(bytes, fields[j].1) { Some((s, _)) => s, None => return false };
            if key == "type" { kind = value; } else if key == "text" { text = value; } else if key == "id" { id = value; } else { name = value; }
        }
        j = j + 1;
    }
    if kind.as_str() == "text" {
        if answer.text.len() > 0 {
            answer.text.push_str("\n");
        }
        answer.text.push_str(&text);
    } else if kind.as_str() == "tool_use" {
        answer.call_ids.push(id);
        answer.call_names.push(name);
        answer.call_args.push(args);
    }
    true
}

// ---- JSON text, written and read over bytes. Every loop walks an index forward toward the end of its bytes, so each
// one ends; an index that would not move forward ends the read as unreadable. The bytes are the door's, already bounded.

fn digit_text(d: u64) -> &'static str {
    match d {
        0 => "0", 1 => "1", 2 => "2", 3 => "3", 4 => "4", 5 => "5", 6 => "6", 7 => "7", 8 => "8", 9 => "9",
        10 => "a", 11 => "b", 12 => "c", 13 => "d", 14 => "e", 15 => "f",
        _ => "0",
    }
}

fn power_of_ten(k: u64) -> u64 {
    match k {
        0 => 1, 1 => 10, 2 => 100, 3 => 1_000, 4 => 10_000, 5 => 100_000, 6 => 1_000_000, 7 => 10_000_000,
        8 => 100_000_000, _ => 1_000_000_000,
    }
}

// n in decimal, at least one digit. n is at most 9_999_999_999 by its callers' bounds.
fn push_count(b: &mut String, n: u64) {
    let mut started = false;
    let mut k: u64 = 10;
    while k > 0
        decreases k,
    {
        k = k - 1;
        let p = power_of_ten(k);
        let digit = if p > 0 { (n / p) % 10 } else { 0 };
        if digit != 0 || started || k == 0 {
            b.push_str(digit_text(digit));
            started = true;
        }
    }
}

// What a byte of text is written as inside a JSON string, when it is not written as itself.
fn escape_of(c: u8) -> &'static str {
    if c == 34 { "\\\"" } else if c == 92 { "\\\\" } else if c == 10 { "\\n" } else if c == 13 { "\\r" }
    else if c == 9 { "\\t" } else { "" }
}

// s as one JSON string. Only ASCII bytes are escaped, so every cut falls between two characters.
fn push_json_string(b: &mut String, s: &str) {
    b.push_str("\"");
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut start: usize = 0;
    let mut i: usize = 0;
    while i < n
        invariant n == bytes.len(), start <= i,
        decreases n - i,
    {
        let c = bytes[i];
        let escape = escape_of(c);
        if escape.len() > 0 || c < 32 {
            match s.get(start..i) { Some(t) => b.push_str(t), None => {} }
            if escape.len() > 0 {
                b.push_str(escape);
            } else {
                b.push_str("\\u00");
                b.push_str(digit_text((c / 16) as u64));
                b.push_str(digit_text((c % 16) as u64));
            }
            start = i + 1;
        }
        i = i + 1;
    }
    match s.get(start..n) { Some(t) => b.push_str(t), None => {} }
    b.push_str("\"");
}

fn is_space(b: u8) -> bool {
    b == 32 || b == 10 || b == 13 || b == 9
}

fn skip_ws(bytes: &[u8], at: usize) -> usize {
    let n = bytes.len();
    let mut i = at;
    while i < n && is_space(bytes[i])
        invariant n == bytes.len(),
        decreases n - i,
    {
        i = i + 1;
    }
    i
}

fn hex_value(h: u8) -> Option<u32> {
    if h >= 48 && h <= 57 { Some((h - 48) as u32) }
    else if h >= 97 && h <= 102 { Some((h - 87) as u32) }
    else if h >= 65 && h <= 70 { Some((h - 55) as u32) }
    else { None }
}

// One code point (at most 0xFFFF, four hex digits) in UTF-8. A surrogate half is kept as the replacement character:
// the text is for reading, not for replay.
fn push_code_point(out: &mut Vec<u8>, v: u32) {
    if v < 128 {
        out.push(v as u8);
    } else if v < 2048 {
        out.push((192 + v / 64) as u8);
        out.push((128 + v % 64) as u8);
    } else if v >= 55296 && v <= 57343 {
        out.push(239);
        out.push(191);
        out.push(189);
    } else if v < 65536 {
        out.push((224 + v / 4096) as u8);
        out.push((128 + (v / 64) % 64) as u8);
        out.push((128 + v % 64) as u8);
    }
}

// A JSON string at `at`: its decoded text and the index after its closing quote.
fn read_string(bytes: &[u8], at: usize) -> Option<(String, usize)> {
    let n = bytes.len();
    if at >= n || bytes[at] != 34 {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut i: usize = at + 1;
    while i < n
        invariant n == bytes.len(),
        decreases n - i,
    {
        let b = bytes[i];
        if b == 34 {
            return match String::from_utf8(out) { Ok(s) => Some((s, i + 1)), Err(_) => None };
        }
        if b == 92 {
            if i + 1 >= n {
                return None;
            }
            i = i + 1;
            let e = bytes[i];
            if e == 110 { out.push(10); }
            else if e == 116 { out.push(9); }
            else if e == 114 { out.push(13); }
            else if e == 98 { out.push(8); }
            else if e == 102 { out.push(12); }
            else if e == 117 {
                let mut v: u32 = 0;
                let mut k: u32 = 0;
                let escaped_at = i;
                while k < 4
                    invariant n == bytes.len(), i < n, k <= 4, v < 65536, escaped_at <= i,
                    decreases 4 - k,
                {
                    if i + 1 >= n {
                        return None;
                    }
                    i = i + 1;
                    let d = match hex_value(bytes[i]) { Some(d) => d, None => return None };
                    if v >= 4096 || d >= 16 {
                        return None;
                    }
                    v = v * 16 + d;
                    k = k + 1;
                }
                push_code_point(&mut out, v);
            } else {
                out.push(e);
            }
        } else {
            out.push(b);
        }
        i = i + 1;
    }
    None
}

// The end of the JSON value starting at `at` (exclusive), skipping strings and nested brackets.
fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    let n = bytes.len();
    if at >= n {
        return None;
    }
    let first = bytes[at];
    if first == 34 {
        return match read_string(bytes, at) { Some((_, end)) => Some(end), None => None };
    }
    let mut i: usize = at;
    if first == 123 || first == 91 {
        let mut depth: u64 = 0;
        while i < n
            invariant n == bytes.len(),
            decreases n - i,
        {
            let b = bytes[i];
            if b == 34 {
                let end = match read_string(bytes, i) { Some((_, end)) => end, None => return None };
                if end <= i || end > n {
                    return None;
                }
                i = end;
            } else {
                if b == 123 || b == 91 {
                    if depth >= 1_000_000 {
                        return None;
                    }
                    depth = depth + 1;
                } else if b == 125 || b == 93 {
                    if depth == 0 {
                        return None;
                    }
                    depth = depth - 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                i = i + 1;
            }
        }
        return None;
    }
    // A number, true, false or null: up to the next delimiter.
    while i < n && !(bytes[i] == 44 || bytes[i] == 125 || bytes[i] == 93 || is_space(bytes[i]))
        invariant n == bytes.len(),
        decreases n - i,
    {
        i = i + 1;
    }
    Some(i)
}

fn read_count(bytes: &[u8], at: usize) -> u64 {
    let n = bytes.len();
    let mut count: u64 = 0;
    let mut i: usize = at;
    while i < n && bytes[i] >= 48 && bytes[i] <= 57
        invariant n == bytes.len(),
        decreases n - i,
    {
        if count >= 1_000_000_000_000 {
            return 0;
        }
        count = count * 10 + (bytes[i] - 48) as u64;
        i = i + 1;
    }
    count
}

// The fields of the object at `at`: each key, where its value starts and where it ends.
fn object_fields(bytes: &[u8], at: usize) -> Option<Vec<(String, usize, usize)>> {
    let n = bytes.len();
    if at >= n || bytes[at] != 123 {
        return None;
    }
    let mut fields: Vec<(String, usize, usize)> = Vec::new();
    let mut i = skip_ws(bytes, at + 1);
    if i < n && bytes[i] == 125 {
        return Some(fields);
    }
    while i < n
        invariant n == bytes.len(),
        decreases n - i,
    {
        let (key, after) = match read_string(bytes, i) { Some(read) => read, None => return None };
        let colon = skip_ws(bytes, after);
        if colon >= n || bytes[colon] != 58 {
            return None;
        }
        let vat = skip_ws(bytes, colon + 1);
        let vend = match value_end(bytes, vat) { Some(end) => end, None => return None };
        fields.push((key, vat, vend));
        let next = skip_ws(bytes, vend);
        if next >= n || next <= i {
            return None;
        }
        if bytes[next] == 125 {
            return Some(fields);
        }
        if bytes[next] != 44 {
            return None;
        }
        i = skip_ws(bytes, next + 1);
        if i <= next || i > n {
            return None;
        }
    }
    None
}

// The items of the array at `at`: where each starts and ends.
fn array_items(bytes: &[u8], at: usize) -> Option<Vec<(usize, usize)>> {
    let n = bytes.len();
    if at >= n || bytes[at] != 91 {
        return None;
    }
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut i = skip_ws(bytes, at + 1);
    if i < n && bytes[i] == 93 {
        return Some(items);
    }
    while i < n
        invariant n == bytes.len(),
        decreases n - i,
    {
        let end = match value_end(bytes, i) { Some(end) => end, None => return None };
        items.push((i, end));
        let next = skip_ws(bytes, end);
        if next >= n || next <= i {
            return None;
        }
        if bytes[next] == 93 {
            return Some(items);
        }
        if bytes[next] != 44 {
            return None;
        }
        i = skip_ws(bytes, next + 1);
        if i <= next || i > n {
            return None;
        }
    }
    None
}

// A door answer's body as text; a body that is not UTF-8 reads as empty, which every reader takes as no answer.
fn text_of(raw: Vec<u8>) -> String {
    match String::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => String::new(),
    }
}
