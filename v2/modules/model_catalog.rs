// model_catalog does one thing: it reads OpenRouter's list of models that take tool calls (TemperPaw's agents need
// them) and answers it as one line per model, so a person can pick any of them:
//   <id> TAB <name> TAB <context length> TAB <prompt price per token> TAB <completion price per token>
// The list is public, so the call carries no key. OpenRouter's answer is about 800 KB (measured 27 Nov: 389 models);
// the lines are about 30 KB. They stop before 120,000 bytes, inside the 128 KiB Temper keeps whole in one field.

const MOST_BYTES: usize = 120_000;

pub fn call(input: Input) -> Option<Answer> {
    let _ = input;
    let reply = door_call(DoorRequest {
        destination: DoorDestination::openrouter, method: DoorMethod::Get, path: "models?supported_parameters=tools".to_string(),
        headers: Vec::new(), body: Vec::new(),
    });
    // Until the first list arrives on V2, every way a read can end is answered as a reason with count 0, so the Catalog
    // record says which one it was.
    let body = match reply {
        DoorReply::Answered(heard) => {
            let DoorAnswer { status, headers: _, body: raw } = heard;
            if status < 200 || status > 299 {
                let mut why = String::new();
                why.push_str("error: openrouter answered status ");
                push_count(&mut why, status as u64);
                why.push_str(": ");
                why.push_str(&ascii_head(&raw, 300));
                return Some(Answer { models: why, count: 0 });
            }
            text_of(raw)
        }
        DoorReply::Failed(n) => {
            let mut why = String::new();
            why.push_str("error: door failed ");
            push_count(&mut why, n as u64);
            return Some(Answer { models: why, count: 0 });
        }
        DoorReply::Refused => return Some(Answer { models: "error: door refused".to_string(), count: 0 }),
        DoorReply::Unknown => return Some(Answer { models: "error: door unknown".to_string(), count: 0 }),
        DoorReply::TooLarge(big) => {
            let mut why = String::new();
            why.push_str("error: reply too large, status ");
            push_count(&mut why, big.status as u64);
            why.push_str(", bytes ");
            push_count(&mut why, big.bytes as u64);
            return Some(Answer { models: why, count: 0 });
        }
    };
    let bytes = body.as_bytes();
    let top = match object_fields(bytes, skip_ws(bytes, 0)) {
        Some(f) => f,
        None => return Some(Answer { models: "error: reply is not a JSON object".to_string(), count: 0 }),
    };
    let fields = top.len();
    let mut j: usize = 0;
    while j < fields
        invariant fields == top.len(),
        decreases fields - j,
    {
        if top[j].0.as_str() == "data" {
            let items = match array_items(bytes, top[j].1) { Some(items) => items, None => return None };
            return Some(lines_of(bytes, &items));
        }
        j = j + 1;
    }
    None
}

// The lines go out oldest model first, the reverse of OpenRouter's order (the page sorts them its own way).
fn lines_of(bytes: &[u8], items: &Vec<(usize, usize)>) -> Answer {
    let mut kept: Vec<String> = Vec::new();
    let mut used: usize = 0;
    let mut count: usize = 0;
    let n = items.len();
    let mut full = false;
    let mut i: usize = 0;
    while i < n && !full
        invariant n == items.len(), i <= n, count <= i, used <= MOST_BYTES,
        decreases n - i,
    {
        let mut line = String::new();
        if model_line(bytes, items[i].0, &mut line) {
            let size = line.len();
            if size > MOST_BYTES - used {
                full = true;
            } else {
                kept.push(line);
                used = used + size;
                count = count + 1;
            }
        }
        i = i + 1;
    }
    let mut models = String::new();
    let mut k: usize = kept.len();
    while k > 0
        invariant k <= kept.len(),
        decreases k,
    {
        k = k - 1;
        models.push_str(&kept[k]);
    }
    Answer { models, count: count as i64 }
}

// One model's line, or false when it has no id.
fn model_line(bytes: &[u8], at: usize, line: &mut String) -> bool {
    let fields = match object_fields(bytes, at) { Some(f) => f, None => return false };
    let mut id = String::new();
    let mut name = String::new();
    let mut context: u64 = 0;
    let mut prompt = String::new();
    let mut completion = String::new();
    let n = fields.len();
    let mut k: usize = 0;
    while k < n
        invariant n == fields.len(),
        decreases n - k,
    {
        let key = fields[k].0.as_str();
        let value = fields[k].1;
        if key == "id" {
            match read_string(bytes, value) { Some((s, _)) => id = s, None => {} }
        } else if key == "name" {
            match read_string(bytes, value) { Some((s, _)) => name = s, None => {} }
        } else if key == "context_length" {
            context = read_count(bytes, value);
        } else if key == "pricing" {
            match object_fields(bytes, value) {
                Some(prices) => {
                    let m = prices.len();
                    let mut p: usize = 0;
                    while p < m
                        invariant m == prices.len(),
                        decreases m - p,
                    {
                        let which = prices[p].0.as_str();
                        if which == "prompt" {
                            match read_string(bytes, prices[p].1) { Some((s, _)) => prompt = s, None => {} }
                        } else if which == "completion" {
                            match read_string(bytes, prices[p].1) { Some((s, _)) => completion = s, None => {} }
                        }
                        p = p + 1;
                    }
                }
                None => {}
            }
        }
        k = k + 1;
    }
    if id.len() == 0 {
        return false;
    }
    line.push_str(&id);
    line.push_str("\t");
    line.push_str(&name);
    line.push_str("\t");
    push_count(line, context);
    line.push_str("\t");
    line.push_str(&prompt);
    line.push_str("\t");
    line.push_str(&completion);
    line.push_str("\n");
    true
}

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

// The first `most` bytes of a reply, printable ASCII only (anything else becomes '?'), for an error's reason.
fn ascii_head(raw: &Vec<u8>, most: usize) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut i: usize = 0;
    while i < raw.len() && i < most
        invariant
            i <= raw.len(),
        decreases raw.len() - i,
    {
        let b = raw[i];
        if b >= 32 && b < 127 {
            kept.push(b);
        } else {
            kept.push(63);
        }
        i = i + 1;
    }
    match String::from_utf8(kept) {
        Ok(text) => text,
        Err(_) => String::new(),
    }
}

// What a byte of text is written as inside a JSON string, when it is not written as itself.
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
