//! Generated: observe one Railway resource.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    dsf_resource_collect::guest::run::<dsf_resource_collect::Railway>()
}
