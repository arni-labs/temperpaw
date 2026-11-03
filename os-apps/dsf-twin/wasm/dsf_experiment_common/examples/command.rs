//! Print the same bounded Exec command used by the WASM for local verification.
use dsf_experiment_common::{Phase, command};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: command validate|run|cleanup MANIFEST RUNNER_SHA256".into());
    }
    let phase = match args[1].as_str() {
        "validate" => Phase::Validate,
        "run" => Phase::Run,
        "cleanup" => Phase::Cleanup,
        _ => return Err("invalid phase".into()),
    };
    println!(
        "{}",
        command(phase, &std::fs::read_to_string(&args[2])?, &args[3])?
    );
    Ok(())
}
