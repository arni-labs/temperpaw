//! A phase prepares one native Exec or reads that same Exec. No transition dispatch.
use base64::{Engine, engine::general_purpose::STANDARD};
use dsf_resource_common::{Callback, Error, Host, Runtime, field, full_sha, identifier, required};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[cfg(target_arch = "wasm32")]
pub mod guest;

#[derive(Clone, Copy)]
pub enum Phase {
    Validate,
    Run,
    Cleanup,
    Select,
}
impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Run => "run",
            Self::Cleanup => "cleanup",
            Self::Select => "select",
        }
    }
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Validate => "Validation",
            Self::Run => "Run",
            Self::Cleanup => "Cleanup",
            Self::Select => "Selection",
        }
    }
    fn waiting(self) -> &'static str {
        match self {
            Self::Validate => "Validating",
            Self::Run => "Running",
            Self::Cleanup => "Cleaning",
            Self::Select => "Selecting",
        }
    }
    /// The counter this phase's callbacks are fenced against. Selection runs
    /// after the Exec phases have already advanced `operation_sequence`, so it
    /// carries its own fence instead of a third step of the shared one.
    pub fn sequence_field(self) -> &'static str {
        match self {
            Self::Validate | Self::Run | Self::Cleanup => "operation_sequence",
            Self::Select => "selection_sequence",
        }
    }
}

pub struct Invocation {
    pub id: String,
    pub sequence: u64,
    pub state: Value,
}
impl Invocation {
    pub fn parse(id: &str, state: &Value, phase: Phase) -> Result<Self, Error> {
        identifier(id)?;
        let sequence = field(state, phase.sequence_field())
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(Error::Binding("missing experiment sequence"))?;
        Ok(Self {
            id: id.into(),
            sequence,
            state: state.clone(),
        })
    }
    pub fn callback(&self, action: &str, mut params: Value) -> Callback {
        params["expected_sequence"] = json!(self.sequence);
        Callback {
            action: action.into(),
            params,
        }
    }
    pub fn failed(&self, phase: Phase, error: Error) -> Callback {
        let suffix = match error {
            Error::ProviderFailed(_) | Error::Proof(_) => "Failed",
            Error::Binding(_) | Error::Field(_) | Error::Response(_)
                if field(&self.state, "status").and_then(Value::as_str)
                    == Some(&format!("{}Preparing", phase.prefix()))
                    && field(&self.state, "exec_id").and_then(Value::as_str)
                        != Some(&execution_id(self, phase)) =>
            {
                "Failed"
            }
            _ => "Uncertain",
        };
        self.callback(
            &format!("{}{suffix}", phase.prefix()),
            json!({"error_message":error.to_string()}),
        )
    }
}

fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn hash(raw: &str) -> bool {
    raw.len() == 64 && full_sha(raw)
}

/// Exact immutable File bytes, then typed field equality with the experiment row.
pub fn validate_manifest(inv: &Invocation, raw: &str) -> Result<Value, Error> {
    if digest(raw.as_bytes()) != required(&inv.state, "manifest_sha256")? {
        return Err(Error::Binding("experiment manifest hash changed"));
    }
    let manifest: Value =
        serde_json::from_str(raw).map_err(|_| Error::Response("experiment manifest"))?;
    if manifest["version"] != 1
        || manifest["experiment_id"] != inv.id
        || manifest["permitted_external_calls"] != json!([])
    {
        return Err(Error::Binding(
            "manifest belongs elsewhere or permits external calls",
        ));
    }
    for name in [
        "effort_id",
        "computer_id",
        "branch",
        "source_revision",
        "database_id",
        "media_bucket",
        "media_namespace",
    ] {
        if required(&manifest, name)? != required(&inv.state, name)? {
            return Err(Error::Binding(
                "manifest and immutable experiment bindings differ",
            ));
        }
    }
    if required(&inv.state, "permitted_external_calls")? != "[]"
        || !full_sha(required(&manifest, "source_revision")?)
        || required(&manifest, "database_id")? == required(&manifest, "production_database_id")?
        || required(&manifest, "media_bucket")? == required(&manifest, "production_media_bucket")?
    {
        return Err(Error::Binding(
            "experiment isolation contract does not pass",
        ));
    }
    Ok(manifest)
}

pub fn execution_id(inv: &Invocation, phase: Phase) -> String {
    format!(
        "dsf-exp-{}",
        &digest(format!("{}:{}:{}", inv.id, phase.name(), inv.sequence).as_bytes())[..40]
    )
}

pub fn command(phase: Phase, raw: &str, runner_sha: &str) -> Result<String, Error> {
    if !hash(runner_sha) {
        return Err(Error::Binding("runner archive requires a pinned SHA256"));
    }
    // All interpolated values are an enum, hex, or Base64. The archive's bytes are verified before imports.
    let path = format!("/home/tl-user/work/arn467-experiments/tools/{runner_sha}.pyz");
    Ok(format!(
        "sudo -n unshare --net -- env -i PATH=/usr/bin:/bin /home/tl-user/work/arn467-experiments/tools/venv/bin/python -I -c 'import hashlib,pathlib,runpy,sys; p=\"{path}\"; h=hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest(); exec(\"if h != \\\"{runner_sha}\\\": raise RuntimeError(\\\"runner archive changed\\\")\"); sys.argv=[p,\"{}\",\"{}\"]; runpy.run_path(p,run_name=\"__main__\")'",
        phase.name(),
        STANDARD.encode(raw)
    ))
}

pub fn execute(
    runtime: &mut Runtime<impl Host>,
    inv: &Invocation,
    phase: Phase,
) -> Result<Callback, Error> {
    let current = runtime.row("DsfExperiments", &inv.id)?;
    if current != inv.state {
        // Field equality is sufficient; read timestamps and representation may differ.
        for name in [
            "operation_sequence",
            "selection_sequence",
            "manifest_ref",
            "manifest_sha256",
            "status",
            "selection_ask_id",
            "delivery_effort_id",
        ] {
            if field(&current, name) != field(&inv.state, name) {
                return Err(Error::Binding("experiment advanced during invocation"));
            }
        }
    }
    if matches!(phase, Phase::Select) {
        return select(runtime, inv);
    }
    let raw = runtime.read("Files", required(&inv.state, "manifest_ref")?, true)?;
    if raw.status != 200 || raw.body.len() > 16384 {
        return Err(Error::Response("experiment manifest File"));
    }
    let manifest = validate_manifest(inv, &raw.body)?;
    let runner_sha = required(&manifest, "runner_sha256")?;
    if !hash(runner_sha) {
        return Err(Error::Binding("manifest runner SHA256 is invalid"));
    }
    let status = required(&inv.state, "status")?;
    let id = execution_id(inv, phase);
    if status == format!("{}Preparing", phase.prefix()) {
        let expected_command = command(phase, &raw.body, runner_sha)?;
        let existing = runtime.read("Execs", &id, false)?;
        let suffix = match existing.status {
            404 => "Prepared",
            200 => {
                let row: Value = serde_json::from_str(&existing.body)
                    .map_err(|_| Error::Response("existing Exec"))?;
                let created = required(&row, "status")? == "Created";
                for (name, expected) in [
                    ("computer_id", required(&inv.state, "computer_id")?),
                    ("command", expected_command.as_str()),
                ] {
                    let actual = field(&row, name).and_then(Value::as_str).unwrap_or("");
                    if actual != expected && !(created && actual.is_empty()) {
                        return Err(Error::Binding("existing Exec belongs elsewhere"));
                    }
                }
                if created { "Prepared" } else { "Reconciled" }
            }
            status => return Err(Error::Http(status, "existing Exec")),
        };
        let budget_ms = if matches!(phase, Phase::Run) {
            1_800_000
        } else {
            300_000
        };
        return Ok(inv.callback(&format!("{}{suffix}",phase.prefix()), json!({"exec_id":id,"command":expected_command,"phase_deadline_ms":(runtime.now_ms+budget_ms).to_string()})));
    }
    if status != phase.waiting() || required(&inv.state, "exec_id")? != id {
        return Err(Error::Binding("phase does not own this Exec"));
    }
    let deadline = required(&inv.state, "phase_deadline_ms")?
        .parse::<i64>()
        .map_err(|_| Error::Binding("invalid phase deadline"))?;
    if runtime.now_ms >= deadline {
        return Err(Error::Pending(
            "phase deadline elapsed; resume reconciles the same Exec",
        ));
    }
    let exec = runtime.row("Execs", &id)?;
    if required(&exec, "computer_id")? != required(&inv.state, "computer_id")?
        || required(&exec, "command")? != command(phase, &raw.body, runner_sha)?
    {
        return Err(Error::Binding("Exec computer or command changed"));
    }
    match required(&exec, "status")? {
        "Created" | "Starting" | "Running" => Ok(Callback {
            action: String::new(),
            params: json!({}),
        }),
        "Failed" => {
            if field(&exec, "exit_code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
            {
                Err(Error::Pending(
                    "Exec outcome is unknown; inspect retained logs",
                ))
            } else {
                Err(Error::ProviderFailed(
                    "experiment Exec failed; inspect retained logs",
                ))
            }
        }
        "Succeeded" => {
            if required(&exec, "exit_code")? != "0" {
                return Err(Error::Proof("Exec did not exit successfully".into()));
            }
            receipt(inv, phase, &manifest, required(&exec, "stdout_tail")?, &id)
        }
        _ => Err(Error::Response("Exec status")),
    }
}

pub fn receipt(
    inv: &Invocation,
    phase: Phase,
    manifest: &Value,
    raw: &str,
    exec_id: &str,
) -> Result<Callback, Error> {
    let proof: Value =
        serde_json::from_str(raw).map_err(|_| Error::Response("experiment receipt"))?;
    if proof["manifest_sha256"] != digest(manifest.to_string().as_bytes())
        || proof["experiment_id"] != inv.id
        || proof["phase"] != phase.name()
        || proof["source_revision"] != manifest["source_revision"]
        || proof["outcome"] != "passed"
    {
        return Err(Error::Proof(
            "receipt does not match the experiment and phase".into(),
        ));
    }
    let reference = format!("Execs('{exec_id}')");
    match phase {
        Phase::Validate => {
            for name in [
                "database_id",
                "media_bucket",
                "media_namespace",
                "branch",
                "production_database_id",
                "production_media_bucket",
            ] {
                if proof[name] != manifest[name] {
                    return Err(Error::Proof("observed isolation target differs".into()));
                }
            }
            for name in [
                "database_system_identifier",
                "database_oid",
                "pgvector_version",
            ] {
                required(&proof, name)?;
            }
            if proof["network_interfaces"] != json!(["lo"])
                || proof["external_routes"] != json!([])
                || proof["external_calls"] != json!([])
            {
                return Err(Error::Proof("network isolation is not verified".into()));
            }
            Ok(inv.callback("IsolationSucceeded",json!({"production_database_id":proof["production_database_id"],"production_media_bucket":proof["production_media_bucket"],"isolation_evidence_ref":reference})))
        }
        Phase::Run => {
            if proof["paid_provider_calls"] != 0
                || proof["external_calls"] != json!([])
                || proof["check_count"].as_u64().unwrap_or(0) < 30
            {
                return Err(Error::Proof(
                    "bounded real application checks did not pass".into(),
                ));
            }
            Ok(inv.callback(
                "RunSucceeded",
                json!({"result_ref":reference,"test_evidence_ref":reference}),
            ))
        }
        Phase::Cleanup => {
            if proof["deleted"]
                != json!([
                    "source",
                    "branch",
                    "postgres",
                    "media",
                    "fixture_credentials"
                ])
            {
                return Err(Error::Proof(
                    "cleanup did not verify every owned artifact".into(),
                ));
            }
            Ok(inv.callback(
                "CleanupSucceeded",
                json!({"cleanup_evidence_ref":reference}),
            ))
        }
        Phase::Select => Err(Error::Binding("selection does not use Exec")),
    }
}

fn select(runtime: &mut Runtime<impl Host>, inv: &Invocation) -> Result<Callback, Error> {
    if required(&inv.state, "status")? != "Selecting" {
        return Err(Error::Binding("selection phase changed"));
    }
    let ask_id = required(&inv.state, "selection_ask_id")?;
    let ask = runtime.row("Asks", ask_id)?;
    if required(&ask, "status")? != "Answered"
        || required(&ask, "effort_id")? != required(&inv.state, "effort_id")?
        || required(&ask, "chose")? != inv.id
    {
        return Err(Error::Proof(
            "selection needs an Answered same-Effort Ask choosing this experiment".into(),
        ));
    }
    let delivery = required(&inv.state, "delivery_effort_id")?;
    let effort = runtime.row("Efforts", delivery)?;
    let intent = runtime.row("Intents", required(&effort, "intent_id")?)?;
    if required(&intent, "status")? != "Accepted" || required(&intent, "effort_id")? != delivery {
        return Err(Error::Proof(
            "promotion must target an ordinary accepted delivery Effort".into(),
        ));
    }
    Ok(inv.callback(
        "SelectionSucceeded",
        json!({"selection_evidence_ref":format!("Asks('{ask_id}') -> Efforts('{delivery}')")}),
    ))
}

#[cfg(test)]
mod tests;
