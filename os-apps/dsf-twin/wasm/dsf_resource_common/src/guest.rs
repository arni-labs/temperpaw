//! Each WASM supplies one concrete resource action and one integration function.
use crate::*;
use temper_wasm_sdk::{Context, set_error_result, set_success_result};

pub struct Guest(Context);
impl Host for Guest {
    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        self.0
            .http_call(
                request.method,
                &request.url,
                &request.headers,
                &request.body,
            )
            .map(|r| Response {
                status: r.status,
                body: r.body,
            })
            .map_err(|_| Error::Transport)
    }
    fn secret(&mut self, name: &str) -> Result<String, Error> {
        identifier(name)?;
        self.0.get_secret(name).map_err(|_| Error::Transport)
    }
}

pub enum Failure {
    Validation,
    Execution,
    Observation,
    Verification,
}

pub fn run<A: ResourceAction>(
    adapter: impl FnOnce(&mut Runtime<Guest>, &Invocation) -> Result<Callback, Error>,
    failure: Failure,
) -> i32 {
    let mut captured = None;
    let result = (|| {
        let ctx = Context::from_host().map_err(|_| Error::Response("invocation context"))?;
        if ctx.entity_type.rsplit('.').next() != Some(A::ENTITY_TYPE) {
            return Err(Error::Binding("WASM belongs to another resource type"));
        }
        let invocation = Invocation::parse(&ctx.entity_id, &ctx.entity_state)?;
        captured = Some(invocation.clone());
        let base = ctx
            .config
            .get("temper_api_url")
            .cloned()
            .ok_or(Error::Binding("missing Temper URL"))?;
        let tenant = ctx.tenant.clone();
        let mut guest = Guest(ctx);
        adapter(
            &mut Runtime {
                host: &mut guest,
                base: &base,
                tenant: &tenant,
                now_ms: Context::get_time_millis(),
            },
            &invocation,
        )
    })();
    match result {
        Ok(callback) => set_success_result(&callback.action, &callback.params),
        Err(error) => match captured {
            Some(invocation) => {
                let callback = failure_callback::<A>(&invocation, &failure, error);
                set_success_result(&callback.action, &callback.params);
            }
            None => set_error_result(
                "resource invocation could not be decoded; no uncorrelated callback emitted",
            ),
        },
    }
    0
}

fn failure_callback<A: ResourceAction>(
    invocation: &Invocation,
    phase: &Failure,
    error: Error,
) -> Callback {
    if let (Failure::Validation, Error::Blocked(ask_id)) = (phase, &error) {
        return invocation.callback::<A>(
            "ValidationBlocked",
            json!({"ask_id":ask_id,
            "error_message":"linked required decision is unresolved or denied"}),
        );
    }
    let suffix = match (phase, &error) {
        (Failure::Validation, _) => "ValidationFailed",
        (Failure::Execution, _) => "ExecutionUncertain",
        (Failure::Observation, _) => "ObservationFailed",
        (Failure::Verification, Error::ProviderFailed(_)) => "VerificationFailed",
        (Failure::Verification, _) => "VerificationPending",
    };
    invocation.callback::<A>(suffix, json!({"error_message":error.to_string()}))
}
