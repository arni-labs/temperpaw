use crate::Error;
use serde_json::Value;

pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
pub struct Response {
    pub status: u16,
    pub body: String,
}
pub trait Host {
    fn request(&mut self, request: &Request) -> Result<Response, Error>;
    fn secret(&mut self, name: &str) -> Result<String, Error>;
}
pub struct Runtime<'a, H> {
    pub host: &'a mut H,
    pub base: &'a str,
    pub tenant: &'a str,
    pub now_ms: i64,
}

pub fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    let value = value.get("fields").unwrap_or(value);
    let pascal: String = name
        .split('_')
        .map(|s| {
            let mut chars = s.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect();
    value.get(name).or_else(|| value.get(pascal))
}
/// Read a declared counter from entity state.
///
/// The kernel materializes a counter only on its first increment, so a counter
/// still holding its declared `initial = "0"` is absent from the serialized
/// state entirely. Absence is therefore the declared initial, zero. A value
/// that is present but is not an unsigned integer remains a binding error.
pub fn counter(value: &Value, name: &str) -> Result<u64, Error> {
    match field(value, name) {
        None => Ok(0),
        Some(present) => present.as_u64().ok_or_else(|| Error::Field(name.into())),
    }
}
pub fn required<'a>(value: &'a Value, name: &str) -> Result<&'a str, Error> {
    field(value, name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Field(name.into()))
}
pub fn decoded(value: &Value, name: &str) -> Result<Value, Error> {
    let value = field(value, name).ok_or_else(|| Error::Field(name.into()))?;
    match value.as_str() {
        Some(raw) => serde_json::from_str(raw).map_err(|_| Error::Field(name.into())),
        None => Ok(value.clone()),
    }
}
pub fn identifier(raw: &str) -> Result<&str, Error> {
    if raw.is_empty()
        || raw.len() > 160
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        Err(Error::Binding("invalid identifier"))
    } else {
        Ok(raw)
    }
}
pub fn full_sha(raw: &str) -> bool {
    [40, 64].contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn encoded(raw: &str) -> String {
    raw.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn json_body(response: Response, source: &'static str) -> Result<Value, Error> {
    if !(200..300).contains(&response.status) {
        return Err(Error::Http(response.status, source));
    }
    if response.body.len() > 1_048_576 {
        return Err(Error::Response(source));
    }
    serde_json::from_str(&response.body).map_err(|_| Error::Response(source))
}

impl<H: Host> Runtime<'_, H> {
    pub fn read(&mut self, set: &str, id: &str, file: bool) -> Result<Response, Error> {
        identifier(set)?;
        identifier(id)?;
        if !self.base.starts_with("https://") && !self.base.starts_with("http://127.0.0.1:") {
            return Err(Error::Binding("invalid Temper base"));
        }
        self.host.request(&Request {
            method: "GET",
            url: format!(
                "{}/tdata/{set}('{id}'){}",
                self.base.trim_end_matches('/'),
                if file { "/$value" } else { "" }
            ),
            headers: vec![("x-tenant-id".into(), self.tenant.into())],
            body: String::new(),
        })
    }
    pub fn row(&mut self, set: &str, id: &str) -> Result<Value, Error> {
        json_body(self.read(set, id, false)?, "Temper")
    }
    pub fn credential(&mut self, name: &str) -> Result<String, Error> {
        identifier(name)?;
        let secret = self.host.secret(name)?;
        if secret.is_empty() || secret.contains(['\r', '\n']) {
            return Err(Error::Binding("invalid credential"));
        }
        Ok(secret)
    }
    /// Provider modules supply a fixed origin and encode each target component.
    pub fn bearer_json(
        &mut self,
        secret_name: &str,
        method: &'static str,
        url: String,
        body: Value,
    ) -> Result<Value, Error> {
        let secret = self.credential(secret_name)?;
        let value = json_body(
            self.host.request(&Request {
                method,
                url,
                headers: vec![
                    ("authorization".into(), format!("Bearer {secret}")),
                    ("content-type".into(), "application/json".into()),
                ],
                body: if method == "GET" || method == "HEAD" {
                    String::new()
                } else {
                    body.to_string()
                },
            })?,
            "provider",
        )?;
        if value
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|e| !e.is_empty())
        {
            return Err(Error::Response("provider errors"));
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RecordingHost(Vec<Request>);
    impl Host for RecordingHost {
        fn request(&mut self, request: &Request) -> Result<Response, Error> {
            self.0.push(Request {
                method: request.method,
                url: request.url.clone(),
                headers: request.headers.clone(),
                body: request.body.clone(),
            });
            Ok(Response {
                status: 200,
                body: "{}".into(),
            })
        }
        fn secret(&mut self, name: &str) -> Result<String, Error> {
            assert_eq!(name, "dsf_railway_token");
            Ok("provider-only-test-credential".into())
        }
    }
    #[test]
    fn local_record_reads_delegate_authority_while_provider_requests_keep_their_credential() {
        let mut host = RecordingHost(vec![]);
        let mut runtime = Runtime {
            host: &mut host,
            base: "http://127.0.0.1:3571",
            tenant: "default",
            now_ms: 1,
        };
        runtime.read("DsfFlows", "flow", false).unwrap();
        runtime
            .bearer_json(
                "dsf_railway_token",
                "POST",
                "https://backboard.railway.com/graphql/v2".into(),
                serde_json::json!({"query":"query ReadOnlyFixture { __typename }"}),
            )
            .unwrap();
        assert!(
            !host.0[0]
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        );
        assert!(
            host.0[1]
                .headers
                .iter()
                .any(|(name, value)| name == "authorization"
                    && value == "Bearer provider-only-test-credential")
        );
    }
}
