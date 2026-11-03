//! Generated: media RetrySelected, validate stage only.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    dsf_resource_common::guest::run::<dsf_media_actions::RetrySelected>(
        dsf_resource_common::validate::<dsf_media_actions::RetrySelected>,
        dsf_resource_common::guest::Failure::Validation,
    )
}
