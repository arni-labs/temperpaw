//! cleanup phase only; execution is owned by the native Exec entity.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr:i32,_ctx_len:i32)->i32 {
    dsf_experiment_common::guest::run(dsf_experiment_common::Phase::Cleanup)
}
