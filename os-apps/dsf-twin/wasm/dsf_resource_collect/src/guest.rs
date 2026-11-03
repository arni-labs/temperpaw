//! A generated WASM selects its collector type at compile time.
use super::*;
use temper_wasm_sdk::{Context, set_error_result, set_success_result};
struct Guest(Context);
impl Host for Guest {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        self.0
            .http_call(
                request.method,
                &request.url,
                &request.headers,
                &request.body,
            )
            .map(|response| Response {
                status: response.status,
                body: response.body,
            })
            .map_err(|_| Error::Transport)
    }
    fn secret(&mut self, name: &str) -> Result<String, Error> {
        self.0.get_secret(name).map_err(|_| Error::Transport)
    }
}
pub fn run<C: Collector>() -> i32 {
    let mut sequence = None;
    let result = (|| {
        let context =
            Context::from_host().map_err(|_| Error::Response("resource observation context"))?;
        if context.entity_type.rsplit('.').next() != Some(C::Binding::ENTITY_TYPE) {
            return Err(Error::Binding("collector belongs to another resource type"));
        }
        // Strict on purpose: this value only fences the failure callback and
        // has no zero branch, so an absent counter here is an undecodable
        // collector context rather than a resource that has never refreshed.
        sequence = Some(
            field(&context.entity_state, "refresh_sequence")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Field("refresh_sequence".into()))?,
        );
        let base = context
            .config
            .get("temper_api_url")
            .cloned()
            .ok_or(Error::Binding("missing Temper URL"))?;
        let tenant = context.tenant.clone();
        let id = context.entity_id.clone();
        let captured = context.entity_state.clone();
        collect::<C>(
            &mut Runtime {
                host: &mut Guest(context),
                base: &base,
                tenant: &tenant,
                now_ms: Context::get_time_millis(),
            },
            &id,
            &captured,
        )
    })();
    match result {
        Ok(callback) => set_success_result(&callback.action, &callback.params),
        Err(error) => match sequence {
            Some(sequence) => set_success_result(
                "CollectionFailed",
                &json!({"expected_refresh_sequence":sequence,"error_message":failure_message(&error)}),
            ),
            None => set_error_result("resource observation context could not be decoded"),
        },
    }
    0
}
