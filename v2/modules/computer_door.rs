// computer_door does one thing: it carries out what the spec decided for the session's computer, its own Tensorlake
// sandbox (never the node's). Each op is one short sequence of door calls:
//   exec     make the sandbox if the session has none, resume it if paused, run one bash command, answer its output
//   pause    suspend the sandbox: its disk is kept, and the next exec resumes it (about 0.5 s, measured 26 Nov)
//   end      delete the sandbox
//   refuse   the spec found no grant for the tool: answer why, with no door call
// The sandbox API is `tensorlake` (api.tensorlake.ai) and the process API `tensorlake_process` (the one fixed host
// sandbox.tensorlake.ai, which names the sandbox by the x-tensorlake-sandbox-id header). Their keys are put in place by
// the doors, out of this module's sight. The sandbox id only ever comes from this module's own answer, so a model
// cannot name another sandbox.
//
// Limits, each with its reason: a command runs at most 120 s, inside the process destination's 180 s deadline, so a long
// command ends as a known timeout; each stream keeps its first 32 KiB, so one result fits the model's context; a resume
// is waited on for at most 40 status reads; a sandbox lives at most an hour (the pool's sandbox rule: 1,800 to 3,600 s, never
// without a timeout), paused whenever it idles; one past its hour is gone, and the next command makes a fresh one.

const RUN_SECONDS: u64 = 120;
const KEEP_BYTES: usize = 32 * 1024;
const LIFETIME_SECONDS: u64 = 3_600;
const STATUS_READS: u64 = 40;

pub fn call(input: Input) -> Option<Answer> {
    let op = input.op.as_str();
    if op == "refuse" {
        return Some(said(&input.sandbox, 126, "This agent holds no grant for this tool."));
    }
    if op == "end" {
        if input.sandbox.len() == 0 {
            return Some(said("", 0, ""));
        }
        let status = sandbox_call(DoorMethod::Delete, &input.sandbox, "", String::new());
        return if (status >= 200 && status <= 299) || status == 404 { Some(said(&input.sandbox, 0, "")) } else { None };
    }
    if op == "pause" {
        if input.sandbox.len() == 0 {
            return None;
        }
        let status = sandbox_call(DoorMethod::Post, &input.sandbox, "/suspend", String::new());
        return if status >= 200 && status <= 299 { Some(said(&input.sandbox, 0, "")) } else { None };
    }
    let command = match command_of(&input.args) {
        Some(c) => c,
        None => return Some(said(&input.sandbox, 2, "The tool's input names no command.")),
    };
    let mut sandbox = input.sandbox.clone();
    if sandbox.len() == 0 {
        sandbox = match make(&input.call) { Some(id) => id, None => return None };
    } else if input.paused {
        resume(&sandbox);
    }
    match run(&sandbox, &command, input.unikon) {
        Ran::Answered(mut answer) => {
            answer.sandbox = sandbox;
            Some(answer)
        }
        // Not running (suspended since): resume and run once more.
        Ran::NotRunning => {
            resume(&sandbox);
            match run(&sandbox, &command, input.unikon) {
                Ran::Answered(mut answer) => {
                    answer.sandbox = sandbox;
                    Some(answer)
                }
                _ => None,
            }
        }
        // Gone (ended at its lifetime, or deleted): a new computer, whose workspace starts empty.
        Ran::Gone => {
            let fresh = match make(&input.call) { Some(id) => id, None => return None };
            match run(&fresh, &command, input.unikon) {
                Ran::Answered(mut answer) => {
                    answer.stderr.push_str("(The computer had ended; this ran on a new one with an empty workspace.)\n");
                    answer.sandbox = fresh;
                    Some(answer)
                }
                _ => None,
            }
        }
        Ran::Failed => None,
    }
}

enum Ran {
    Answered(Answer),
    NotRunning,
    Gone,
    Failed,
}

fn said(sandbox: &str, code: i64, why: &str) -> Answer {
    Answer { sandbox: sandbox.to_string(), exit_code: code, stdout: String::new(), stderr: why.to_string() }
}

fn json_headers() -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();
    headers.push(("Content-Type".to_string(), "application/json".to_string()));
    headers
}

// One call to the sandbox API on `sandboxes/<id><rest>`: its status, or 0 when the door gave no answer.
fn sandbox_call(method: DoorMethod, sandbox: &str, rest: &str, body: String) -> u64 {
    let mut path = String::from("sandboxes/");
    path.push_str(sandbox);
    path.push_str(rest);
    let reply = door_call(DoorRequest {
        destination: DoorDestination::tensorlake, method, path, headers: json_headers(), body,
        keys: vec!["tensorlake_key".to_string()],
    });
    match reply {
        DoorReply::Answered(heard) => heard.status,
        _ => 0,
    }
}

// A sandbox name from the first call that needs one: "paw-" and the call id's letters and digits, lower case.
fn name_of(call: &str) -> String {
    let mut name = String::from("paw-");
    let bytes = call.as_bytes();
    let n = bytes.len();
    let mut kept: usize = 0;
    let mut i: usize = 0;
    while i < n && kept < 40
        invariant n == bytes.len(),
        decreases n - i,
    {
        let c = bytes[i];
        if c >= 48 && c <= 57 {
            name.push_str(digit_text((c - 48) as u64));
            kept = kept + 1;
        } else if (c >= 97 && c <= 122) || (c >= 65 && c <= 90) {
            let lower = if c <= 90 { c + 32 } else { c };
            name.push_str(letter_text(lower));
            kept = kept + 1;
        }
        i = i + 1;
    }
    name
}

fn letter_text(c: u8) -> &'static str {
    match c {
        97 => "a", 98 => "b", 99 => "c", 100 => "d", 101 => "e", 102 => "f", 103 => "g", 104 => "h", 105 => "i",
        106 => "j", 107 => "k", 108 => "l", 109 => "m", 110 => "n", 111 => "o", 112 => "p", 113 => "q", 114 => "r",
        115 => "s", 116 => "t", 117 => "u", 118 => "v", 119 => "w", 120 => "x", 121 => "y", _ => "z",
    }
}

// Makes a named sandbox (a name is what lets Tensorlake suspend and resume it) and waits until it runs.
fn make(call: &str) -> Option<String> {
    let mut body = String::from("{\"name\":");
    push_json_string(&mut body, &name_of(call));
    body.push_str(",\"image\":\"tensorlake/ubuntu-minimal\",\"resources\":{\"cpus\":1,\"memory_mb\":2048},\"timeout_secs\":");
    push_count(&mut body, LIFETIME_SECONDS);
    body.push_str("}");
    let reply = door_call(DoorRequest {
        destination: DoorDestination::tensorlake, method: DoorMethod::Post, path: "sandboxes".to_string(), headers: json_headers(),
        body, keys: vec!["tensorlake_key".to_string()],
    });
    let id = match reply {
        DoorReply::Answered(heard) => {
            let DoorAnswer { status, body } = heard;
            if status < 200 || status > 299 {
                return None;
            }
            match sandbox_id_of(&body) { Some(id) => id, None => return None }
        }
        _ => return None,
    };
    if wait_running(&id) { Some(id) } else { None }
}

fn sandbox_id_of(body: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let fields = match object_fields(bytes, skip_ws(bytes, 0)) { Some(f) => f, None => return None };
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        if fields[j].0.as_str() == "sandbox_id" {
            return match read_string(bytes, fields[j].1) { Some((s, _)) => if s.len() > 0 { Some(s) } else { None }, None => None };
        }
        j = j + 1;
    }
    None
}

// A resume sent while the sandbox is still suspending is refused, so it waits for "suspended" and asks again, twice.
fn resume(sandbox: &str) {
    let mut tries: u64 = 0;
    while tries < 3
        decreases 3 - tries,
    {
        let status = sandbox_call(DoorMethod::Post, sandbox, "/resume", String::new());
        if status >= 200 && status <= 299 {
            let _ = wait_running(sandbox);
            return;
        }
        if !wait_for(sandbox, "\"status\":\"suspended\"") {
            return;
        }
        tries = tries + 1;
    }
}

fn wait_running(sandbox: &str) -> bool {
    wait_for(sandbox, "\"status\":\"running\"")
}

// Reads the sandbox until its record says `word`, at most STATUS_READS times.
fn wait_for(sandbox: &str, word: &str) -> bool {
    let mut path = String::from("sandboxes/");
    path.push_str(sandbox);
    let mut k: u64 = 0;
    while k < STATUS_READS
        decreases STATUS_READS - k,
    {
        let reply = door_call(DoorRequest {
            destination: DoorDestination::tensorlake, method: DoorMethod::Get, path: path.clone(), headers: json_headers(),
            body: String::new(), keys: vec!["tensorlake_key".to_string()],
        });
        match reply {
            DoorReply::Answered(heard) => {
                let DoorAnswer { status, body } = heard;
                if status >= 200 && status <= 299 && body.contains(word) {
                    return true;
                }
            }
            _ => {}
        }
        k = k + 1;
    }
    false
}

// With unikon, the door puts the vault's UniKon connector credential in the command's env (UNIKON_MCP_TOKEN), out of
// this module's sight; without it the call names no such key, so an agent without the grant never carries it.
fn run(sandbox: &str, command: &str, unikon: bool) -> Ran {
    let mut body = String::from("{\"command\":\"bash\",\"args\":[\"-lc\",");
    push_json_string(&mut body, command);
    body.push_str("],\"working_dir\":\"/home/tl-user\",\"timeout\":");
    push_count(&mut body, RUN_SECONDS);
    body.push_str(",\"env\":{}}");
    let mut keys = vec!["tensorlake_process_key".to_string()];
    if unikon {
        keys.push("unikon_mcp_token".to_string());
    }
    let mut headers = json_headers();
    headers.push(("x-tensorlake-sandbox-id".to_string(), sandbox.to_string()));
    let reply = door_call(DoorRequest {
        destination: DoorDestination::tensorlake_process, method: DoorMethod::Post, path: "processes/run".to_string(), headers,
        body, keys,
    });
    match reply {
        DoorReply::Answered(heard) => {
            let DoorAnswer { status, body } = heard;
            if status >= 200 && status <= 299 {
                Ran::Answered(read_events(&body))
            } else if status == 404 || body.contains("SANDBOX_NOT_FOUND") || body.contains("terminated") {
                Ran::Gone
            } else if body.contains("SANDBOX_NOT_RUNNING") || status == 409 {
                Ran::NotRunning
            } else {
                Ran::Failed
            }
        }
        _ => Ran::Failed,
    }
}

fn command_of(args: &str) -> Option<String> {
    let bytes = args.as_bytes();
    let fields = match object_fields(bytes, skip_ws(bytes, 0)) { Some(f) => f, None => return None };
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        if fields[j].0.as_str() == "command" {
            return match read_string(bytes, fields[j].1) { Some((s, _)) => Some(s), None => None };
        }
        j = j + 1;
    }
    None
}

// Keeps a line of output while the stream holds less than KEEP_BYTES.
fn keep(into: &mut String, line: &str) {
    let (held, more) = (into.len(), line.len());
    if held < KEEP_BYTES && more < KEEP_BYTES && held + more < KEEP_BYTES {
        into.push_str(line);
        into.push_str("\n");
    }
}

// One event line: `{"line": .., "stream": ..}` or, last, `{"exit_code": ..}`.
fn read_event(line: &str, answer: &mut Answer) {
    let bytes = line.as_bytes();
    let fields = match object_fields(bytes, 0) { Some(f) => f, None => return };
    let mut text = String::new();
    let mut has_text = false;
    let mut on_stderr = false;
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        let at = fields[j].1;
        if key == "line" {
            match read_string(bytes, at) { Some((s, _)) => { text = s; has_text = true; } None => {} }
        } else if key == "stream" {
            match read_string(bytes, at) { Some((s, _)) => on_stderr = s.as_str() == "stderr", None => {} }
        } else if key == "exit_code" {
            answer.exit_code = read_count(bytes, at) as i64;
        }
        j = j + 1;
    }
    if has_text {
        if on_stderr { keep(&mut answer.stderr, &text); } else { keep(&mut answer.stdout, &text); }
    }
}

// One line of the answer, its `data:` taken off.
fn read_line(raw: &str, answer: &mut Answer) {
    let line = raw.trim();
    let b = line.as_bytes();
    let has_data = b.len() >= 5 && b[0] == 100 && b[1] == 97 && b[2] == 116 && b[3] == 97 && b[4] == 58;
    if has_data {
        match line.get(5..) { Some(rest) => read_event(rest.trim(), answer), None => {} }
    } else {
        read_event(line, answer);
    }
}

// The process API answers events, one JSON object a line, some after `data:`. A run with no exit code answers 255.
// Lines are cut at newline bytes, which never fall inside a character.
fn read_events(body: &str) -> Answer {
    let mut answer = Answer { sandbox: String::new(), exit_code: 255, stdout: String::new(), stderr: String::new() };
    let bytes = body.as_bytes();
    let n = bytes.len();
    let mut start: usize = 0;
    let mut i: usize = 0;
    while i < n
        invariant n == bytes.len(), start <= i,
        decreases n - i,
    {
        if bytes[i] == 10 {
            match body.get(start..i) { Some(l) => read_line(l, &mut answer), None => {} }
            start = i + 1;
        }
        i = i + 1;
    }
    match body.get(start..n) { Some(l) => read_line(l, &mut answer), None => {} }
    answer
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
