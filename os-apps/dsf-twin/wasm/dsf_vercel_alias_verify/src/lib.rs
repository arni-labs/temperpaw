//! Generated: vercel SetAlias, verify stage only.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    dsf_resource_common::guest::run::<dsf_vercel_actions::SetAlias>(
        dsf_resource_common::verify::<dsf_vercel_actions::SetAlias>,
        dsf_resource_common::guest::Failure::Verification,
    )
}
