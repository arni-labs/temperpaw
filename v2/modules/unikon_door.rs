// unikon_door does one thing: it makes one UniKon connector call for the model, as the agent's owner, and answers the
// tool's text. The model calls the `unikon` tool with {"tool": "<name>", "arguments": {...}}; this module speaks MCP over
// HTTP to the `unikon` door (https://unikon.dev/mcp): initialize, then notifications/initialized, then tools/call (or
// tools/list, when the tool is "list_tools"). The owner's connector key is put in the Authorization header by the door's
// middleware (door_sign), out of this module's sight and never on the session's computer. No sandbox is involved.
//
// Limits, each with its reason: an answer keeps its first 32 KiB, as a computer's stream does, so one result fits the
// model's context and the session's row (a bigger read goes to the computer: inspect_app part repo, then git); an error
// body keeps 600 bytes, enough to say why.

const KEEP_BYTES: usize = 32 * 1024;
const ERROR_KEEP: usize = 600;
const INITIALIZE: &'static str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"seed-unikon-door\",\"version\":\"1\"}}}";
const INITIALIZED: &'static str = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}";
const LIST_TOOLS: &'static str = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}";
const NO_TOOL: &'static str = "The unikon tool's input names no tool. Send {\"tool\": \"<name>\", \"arguments\": {...}}, or {\"tool\": \"list_tools\"} for the list.";

// Every answer names the call it made, so the spec moves nothing on an answer to a call a stop ended.
pub fn call(input: Input) -> Option<Answer> {
    let call = input.call.clone();
    match connect(input) {
        Some(mut answer) => {
            answer.call = call;
            Some(answer)
        }
        None => None,
    }
}

fn connect(input: Input) -> Option<Answer> {
    let (tool, arguments) = match tool_of(&input.args) {
        Some(found) => found,
        None => return Some(said(2, NO_TOOL)),
    };
    let session = match post(INITIALIZE.to_string(), "") {
        Posted::Answered(_, s) => s,
        Posted::Status(status, body) => return Some(refused(status, &body)),
        Posted::Failed => return None,
    };
    match post(INITIALIZED.to_string(), &session) {
        Posted::Failed => return None,
        _ => {}
    }
    let listing = tool.as_str() == "list_tools";
    let body = if listing {
        LIST_TOOLS.to_string()
    } else {
        let mut b = String::from("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":");
        push_json_string(&mut b, &tool);
        b.push_str(",\"arguments\":");
        b.push_str(&arguments);
        b.push_str("}}");
        b
    };
    match post(body, &session) {
        Posted::Answered(text, _) => Some(answer_of(&text, listing)),
        Posted::Status(status, body) => Some(refused(status, &body)),
        // Unknown or failed: the connector may have acted, so nothing is retried (the door is at most once).
        Posted::Failed => None,
    }
}

enum Posted {
    Answered(String, String),
    Status(u64, String),
    Failed,
}

fn said(code: i64, why: &str) -> Answer {
    Answer { sandbox: String::new(), exit_code: code, stdout: String::new(), stderr: why.to_string(), call: String::new() }
}

// A connector status outside 2xx: exit 1, the status and the start of its body.
fn refused(status: u64, body: &str) -> Answer {
    let m = if status > 999 { 999 } else { status };
    let mut why = String::from("UniKon answered HTTP ");
    why.push_str(digit_text(m / 100));
    why.push_str(digit_text((m / 10) % 10));
    why.push_str(digit_text(m % 10));
    why.push_str(": ");
    why.push_str(&cut_at_ascii(body, ERROR_KEEP));
    said(1, &why)
}

// One POST to the connector: its body and the MCP session it names (the one sent, when it names none).
fn post(body: String, session: &str) -> Posted {
    let mut headers: Vec<(String, String)> = Vec::new();
    headers.push(("content-type".to_string(), "application/json".to_string()));
    headers.push(("accept".to_string(), "application/json, text/event-stream".to_string()));
    headers.push(("mcp-protocol-version".to_string(), "2025-06-18".to_string()));
    if session.len() > 0 {
        headers.push(("mcp-session-id".to_string(), session.to_string()));
    }
    let reply = door_call(DoorRequest {
        destination: DoorDestination::unikon, method: DoorMethod::Post, path: "mcp".to_string(), headers, body: body.into_bytes(),
    });
    match reply {
        DoorReply::Answered(heard) => {
            let DoorAnswer { status, headers: got, body: raw } = heard;
            let text = text_of(raw);
            if status < 200 || status > 299 {
                return Posted::Status(status, text);
            }
            Posted::Answered(text, session_of(&got, session))
        }
        _ => Posted::Failed,
    }
}

fn session_of(headers: &Vec<(String, String)>, sent: &str) -> String {
    let count = headers.len();
    let mut j: usize = 0;
    while j < count
        invariant count == headers.len(),
        decreases count - j,
    {
        let name = headers[j].0.as_str();
        if name == "mcp-session-id" || name == "Mcp-Session-Id" {
            return headers[j].1.clone();
        }
        j = j + 1;
    }
    sent.to_string()
}

// The tool's name and its arguments as JSON text, from what the model wrote: {"tool": .., "arguments": {..}}. Arguments
// the model sent as a string of JSON are taken as that JSON; none are {}.
fn tool_of(args: &str) -> Option<(String, String)> {
    let bytes = args.as_bytes();
    let fields = match object_fields(bytes, skip_ws(bytes, 0)) { Some(f) => f, None => return None };
    let mut tool = String::new();
    let mut arguments = String::from("{}");
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let (start, end) = (fields[j].1, fields[j].2);
        let key = fields[j].0.as_str();
        if key == "tool" || key == "name" {
            match read_string(bytes, start) { Some((s, _)) => tool = s, None => {} }
        } else if key == "arguments" && start < bytes.len() {
            if bytes[start] == 34 {
                match read_string(bytes, start) { Some((s, _)) => arguments = s, None => {} }
            } else if bytes[start] == 123 {
                match args.get(start..end) { Some(raw) => arguments = raw.to_string(), None => {} }
            }
        }
        j = j + 1;
    }
    if tool.len() == 0 { None } else { Some((tool, arguments)) }
}

// Where the JSON-RPC message starts: the body itself, or the last `data:` line of an event stream.
fn message_at(bytes: &[u8]) -> Option<usize> {
    let n = bytes.len();
    let first = skip_ws(bytes, 0);
    if first < n && bytes[first] == 123 {
        return Some(first);
    }
    let mut found: Option<usize> = None;
    let mut i: usize = 0;
    while i < n
        invariant n == bytes.len(),
        decreases n - i,
    {
        if n - i >= 5 && bytes[i] == 100 && bytes[i + 1] == 97 && bytes[i + 2] == 116 && bytes[i + 3] == 97 && bytes[i + 4] == 58 {
            let at = skip_ws(bytes, i + 5);
            if at < n && bytes[at] == 123 {
                found = Some(at);
            }
        }
        i = i + 1;
    }
    found
}

// The connector's answer as the tool's: a call's text parts (exit 1 when it says isError), the tool list's names and
// first lines, or a JSON-RPC error's text.
fn answer_of(text: &str, listing: bool) -> Answer {
    let bytes = text.as_bytes();
    let at = match message_at(bytes) { Some(at) => at, None => return said(1, &cut_at_ascii(text, ERROR_KEEP)) };
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return said(1, &cut_at_ascii(text, ERROR_KEEP)) };
    let mut out = String::new();
    let mut code: i64 = 1;
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let (start, end) = (fields[j].1, fields[j].2);
        let key = fields[j].0.as_str();
        if key == "error" {
            match text.get(start..end) { Some(raw) => out.push_str(raw), None => {} }
        } else if key == "result" {
            code = if listing { read_tools(bytes, start, &mut out) } else { read_result(bytes, start, &mut out) };
        }
        j = j + 1;
    }
    let mut answer = said(0, "");
    answer.exit_code = code;
    if out.len() > KEEP_BYTES {
        answer.stdout = cut_at_ascii(&out, KEEP_BYTES);
        answer.stderr.push_str("(The answer was cut at 32 KiB. For a big read, use inspect_app part repo and git on your computer.)");
    } else {
        answer.stdout = out;
    }
    answer
}

// A tools/call result: its text parts, one after another; 1 when it says isError, else 0.
fn read_result(bytes: &[u8], at: usize, out: &mut String) -> i64 {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return 1 };
    let mut code: i64 = 0;
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let start = fields[j].1;
        let key = fields[j].0.as_str();
        if key == "isError" && start < bytes.len() && bytes[start] == 116 {
            code = 1;
        } else if key == "content" {
            let items = match array_items(bytes, start) { Some(items) => items, None => Vec::new() };
            let total = items.len();
            let mut k: usize = 0;
            while k < total
                invariant total == items.len(),
                decreases total - k,
            {
                push_text_part(bytes, items[k].0, out);
                k = k + 1;
            }
        }
        j = j + 1;
    }
    code
}

fn push_text_part(bytes: &[u8], at: usize, out: &mut String) {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return };
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        if fields[j].0.as_str() == "text" {
            match read_string(bytes, fields[j].1) {
                Some((s, _)) => {
                    if out.len() > 0 {
                        out.push_str("\n");
                    }
                    out.push_str(&s);
                }
                None => {}
            }
        }
        j = j + 1;
    }
}

// A tools/list result: one line per tool, its name and the first line of what it does.
fn read_tools(bytes: &[u8], at: usize, out: &mut String) -> i64 {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return 1 };
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        if fields[j].0.as_str() == "tools" {
            let items = match array_items(bytes, fields[j].1) { Some(items) => items, None => Vec::new() };
            let total = items.len();
            let mut k: usize = 0;
            while k < total
                invariant total == items.len(),
                decreases total - k,
            {
                push_tool_line(bytes, items[k].0, out);
                k = k + 1;
            }
        }
        j = j + 1;
    }
    0
}

fn push_tool_line(bytes: &[u8], at: usize, out: &mut String) {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return };
    let mut name = String::new();
    let mut what = String::new();
    let count = fields.len();
    let mut j: usize = 0;
    while j < count
        invariant count == fields.len(),
        decreases count - j,
    {
        let key = fields[j].0.as_str();
        if key == "name" {
            match read_string(bytes, fields[j].1) { Some((s, _)) => name = s, None => {} }
        } else if key == "description" {
            match read_string(bytes, fields[j].1) { Some((s, _)) => what = first_line(&s), None => {} }
        }
        j = j + 1;
    }
    out.push_str(&name);
    out.push_str(": ");
    out.push_str(&what);
    out.push_str("\n");
}

// The text up to its first newline, at most 200 bytes.
fn first_line(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut end: usize = 0;
    while end < n && end < 200 && bytes[end] != 10
        invariant n == bytes.len(), end <= n,
        decreases n - end,
    {
        end = end + 1;
    }
    cut_at_ascii(s, end)
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

fn digit_text(d: u64) -> &'static str {
    match d {
        0 => "0", 1 => "1", 2 => "2", 3 => "3", 4 => "4", 5 => "5", 6 => "6", 7 => "7", 8 => "8", 9 => "9",
        10 => "a", 11 => "b", 12 => "c", 13 => "d", 14 => "e", 15 => "f",
        _ => "0",
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
