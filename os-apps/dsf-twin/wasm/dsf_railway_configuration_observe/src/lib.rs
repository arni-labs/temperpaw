//! Generated: railway ApplyConfiguration, observe stage only.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    dsf_resource_common::guest::run::<dsf_railway_actions::ApplyConfiguration>(
        dsf_resource_common::observe::<dsf_railway_actions::ApplyConfiguration>,
        dsf_resource_common::guest::Failure::Observation,
    )
}
