// door_sign does one thing: it puts a destination's keys into a request at send time, the one place a key's value is
// handed (S-DOOR-10). It is the middleware of every TemperPaw door. Its parameters, from the destination, are plain:
//   header, prefix   the header that carries `header_key`'s value, written as prefix + value
//   header_key       which key goes in that header
//   body_key         which key, if any, goes into the body
//   body_mark        the text the calling module wrote where that key goes; the last one in the body is replaced
// A body with no mark is sent as it is, so a call that wrote no mark (an agent without the unikon grant) carries no
// second key. The mark is replaced only at its last place, the env slot computer_door writes after the command, so a
// command that happens to hold the mark text is never filled in.

// The index of the last place `mark` starts in `body`, if it does.
fn last_mark(body: &Vec<u8>, mark: &Vec<u8>) -> (found: Option<usize>) {
    let n = body.len();
    let m = mark.len();
    if m == 0 || m > n {
        return None;
    }
    let mut found: Option<usize> = None;
    let mut at: usize = 0;
    while at <= n - m
        invariant
            n == body.len(),
            m == mark.len(),
            0 < m,
            m <= n,
        decreases n - m + 1 - at,
    {
        let mut same = true;
        let mut k: usize = 0;
        while k < m && same
            invariant
                n == body.len(),
                m == mark.len(),
                m <= n,
                at <= n - m,
                k <= m,
            decreases m - k,
        {
            if body[at + k] != mark[k] {
                same = false;
            }
            k = k + 1;
        }
        if same {
            found = Some(at);
        }
        at = at + 1;
    }
    found
}

pub fn call(input: Input) -> (answer: Option<Answer>) {
    let Input { request, params, secrets, now: _, random: _ } = input;
    let mut header: Option<String> = None;
    let mut prefix = String::new();
    let mut header_key = String::new();
    let mut body_key = String::new();
    let mut body_mark = String::new();
    let mut at: usize = 0;
    while at < params.len()
        invariant
            at <= params.len(),
        decreases params.len() - at,
    {
        if params[at].0.as_str() == "header" {
            header = Some(params[at].1.clone());
        } else if params[at].0.as_str() == "prefix" {
            prefix = params[at].1.clone();
        } else if params[at].0.as_str() == "header_key" {
            header_key = params[at].1.clone();
        } else if params[at].0.as_str() == "body_key" {
            body_key = params[at].1.clone();
        } else if params[at].0.as_str() == "body_mark" {
            body_mark = params[at].1.clone();
        }
        at = at + 1;
    }
    let header = match header {
        Some(header) => header,
        None => return None,
    };
    let mut header_value: Option<Vec<u8>> = None;
    let mut body_value: Option<Vec<u8>> = None;
    let mut at: usize = 0;
    while at < secrets.len()
        invariant
            at <= secrets.len(),
        decreases secrets.len() - at,
    {
        if secrets[at].0.as_str() == header_key.as_str() {
            header_value = Some(secrets[at].1.clone());
        } else if body_key.len() > 0 && secrets[at].0.as_str() == body_key.as_str() {
            body_value = Some(secrets[at].1.clone());
        }
        at = at + 1;
    }
    let key = match header_value {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(key) => key,
            Err(_) => return None,
        },
        None => return None,
    };
    let Input_request { method, path, headers, body } = request;
    let mut sent: Vec<Answer_headers_item> = Vec::new();
    let mut at: usize = 0;
    while at < headers.len()
        invariant
            at <= headers.len(),
        decreases headers.len() - at,
    {
        sent.push(Answer_headers_item { name: headers[at].name.clone(), value: headers[at].value.clone() });
        at = at + 1;
    }
    let mut value = prefix;
    value.push_str(key.as_str());
    sent.push(Answer_headers_item { name: header, value });
    let mark: Vec<u8> = body_mark.into_bytes();
    let body = match body_value {
        None => body,
        Some(secret) => match last_mark(&body, &mark) {
            None => body,
            Some(start) => {
                let n = body.len();
                let m = mark.len();
                if start > n || m > n - start {
                    return None;
                }
                let mut made: Vec<u8> = Vec::new();
                let mut i: usize = 0;
                while i < start
                    invariant
                        n == body.len(),
                        start <= n,
                        i <= start,
                    decreases start - i,
                {
                    made.push(body[i]);
                    i = i + 1;
                }
                let mut j: usize = 0;
                while j < secret.len()
                    invariant
                        j <= secret.len(),
                    decreases secret.len() - j,
                {
                    made.push(secret[j]);
                    j = j + 1;
                }
                let mut i: usize = start + m;
                while i < n
                    invariant
                        n == body.len(),
                        i <= n,
                    decreases n - i,
                {
                    made.push(body[i]);
                    i = i + 1;
                }
                made
            }
        },
    };
    Some(Answer { method, path, headers: sent, body })
}
