use super::*;
use dsf_resource_common::{Request, Response};
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
            .map(|r| Response {
                status: r.status,
                body: r.body,
            })
            .map_err(|_| Error::Transport)
    }
    fn secret(&mut self, name: &str) -> Result<String, Error> {
        self.0.get_secret(name).map_err(|_| Error::Transport)
    }
}
pub fn run(phase: Phase) -> i32 {
    let mut captured = None;
    let result = (|| {
        let ctx = Context::from_host().map_err(|_| Error::Response("experiment context"))?;
        if ctx.entity_type.rsplit('.').next() != Some("DsfExperiment") {
            return Err(Error::Binding("module belongs to DsfExperiment"));
        }
        let invocation = Invocation::parse(&ctx.entity_id, &ctx.entity_state, phase)?;
        captured = Some(invocation);
        let invocation = captured
            .as_ref()
            .ok_or(Error::Response("experiment context"))?;
        let base = ctx
            .config
            .get("temper_api_url")
            .cloned()
            .ok_or(Error::Binding("missing Temper URL"))?;
        let tenant = ctx.tenant.clone();
        execute(
            &mut Runtime {
                host: &mut Guest(ctx),
                base: &base,
                tenant: &tenant,
                now_ms: Context::get_time_millis(),
            },
            invocation,
            phase,
        )
    })();
    match result {
        Ok(callback) => set_success_result(&callback.action, &callback.params),
        Err(error) => match captured {
            Some(inv) => {
                let callback = inv.failed(phase, error);
                set_success_result(&callback.action, &callback.params);
            }
            None => set_error_result("experiment context is invalid; no unfenced callback emitted"),
        },
    }
    0
}
