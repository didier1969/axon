use anyhow::{anyhow, Context, Result};
use axon_core::release_reconciler::{
    drive_cutover, evaluate_liveness_gates, evaluate_stop_gates, liveness_next_action,
    liveness_phase, stop_next_action, stop_phase, CutoverIo, CutoverVerdict, LivenessFacts,
    StopFacts,
};
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Phase 0: InstanceConfig — deterministic path computation from
// (project_root, instance_kind, role). Mirrors axon-instance.sh:121-174
// and axon-role-layout.sh:99-142 exactly.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceKind {
    Dev,
    Live,
}

impl InstanceKind {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "dev" => Ok(Self::Dev),
            "live" => Ok(Self::Live),
            _ => Err(anyhow!(
                "invalid --instance-kind: `{s}` (expected dev|live)"
            )),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Live => "live",
        }
    }

    /// Returns the label of the OTHER instance kind.
    /// Used to exclude processes belonging to the opposite instance during stop.
    fn opposite_label(&self) -> &'static str {
        match self {
            Self::Dev => "live",
            Self::Live => "dev",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeRole {
    Brain,
    Indexer,
    All,
}

impl RuntimeRole {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "brain" => Ok(Self::Brain),
            "indexer" => Ok(Self::Indexer),
            "all" => Ok(Self::All),
            _ => Err(anyhow!(
                "invalid --role: `{s}` (expected brain|indexer|all)"
            )),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Brain => "brain",
            Self::Indexer => "indexer",
            Self::All => "all",
        }
    }

    fn binary_name(&self) -> &'static str {
        match self {
            Self::Brain => "axon-brain",
            Self::Indexer => "axon-indexer",
            Self::All => "axon-brain", // unused for All; individual roles are expanded
        }
    }

    /// Expand into concrete roles. All expands to Brain then Indexer.
    fn concrete_roles(&self) -> Vec<RuntimeRole> {
        match self {
            Self::All => vec![RuntimeRole::Brain, RuntimeRole::Indexer],
            other => vec![*other],
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct InstanceConfig {
    project_root: PathBuf,
    instance_kind: InstanceKind,
    role: RuntimeRole,
    tmux_session: String,
    elixir_node_name: String,
    pid_file: PathBuf,
    run_root: PathBuf,
    db_root: PathBuf,
    telemetry_sock: PathBuf,
    mcp_sock: PathBuf,
    runtime_state_file: PathBuf,
    runtime_binary_name: String,
    phx_port: u16,
    hydra_tcp_port: u16,
    hydra_http_port: u16,
    hydra_odata_port: u16,
    hydra_http2_port: u16,
    hydra_mcp_port: u16,
    writer_lock_paths: Vec<(String, PathBuf)>,
}

impl InstanceConfig {
    fn new(project_root: PathBuf, instance_kind: InstanceKind, role: RuntimeRole) -> Self {
        let role_label = role.label();
        let instance_label = instance_kind.label();
        let binary_name = role.binary_name().to_string();

        // Ports: axon-instance.sh lines 136-165
        let (base_port, instance_dir, elixir_node_name) = match instance_kind {
            InstanceKind::Dev => (44137u16, ".axon-dev", "axon_dev_nexus"),
            InstanceKind::Live => (44127u16, ".axon", "axon_nexus"),
        };

        // Role layout: axon-role-layout.sh lines 99-142
        let run_root = project_root
            .join(instance_dir)
            .join(format!("run-{role_label}"));
        let db_root = project_root.join(instance_dir).join("graph_v2");
        let tmux_session = match instance_kind {
            InstanceKind::Dev => format!("axon-dev-{role_label}"),
            InstanceKind::Live => format!("axon-{role_label}"),
        };
        let telemetry_sock = PathBuf::from(format!(
            "/tmp/axon-{instance_label}-{role_label}-telemetry.sock"
        ));
        let mcp_sock = PathBuf::from(format!("/tmp/axon-{instance_label}-{role_label}-mcp.sock"));

        let pid_file = run_root.join(format!("{binary_name}.pid"));
        let runtime_state_file = run_root.join("runtime.env");

        let writer_lock_paths = vec![
            ("IST".to_string(), db_root.join(".axon-ist.writer.lock")),
            ("SOLL".to_string(), db_root.join(".axon-soll.writer.lock")),
        ];

        Self {
            project_root,
            instance_kind,
            role,
            tmux_session,
            elixir_node_name: elixir_node_name.to_string(),
            pid_file,
            run_root,
            db_root,
            telemetry_sock,
            mcp_sock,
            runtime_state_file,
            runtime_binary_name: binary_name,
            phx_port: base_port,
            hydra_tcp_port: base_port + 1,
            hydra_http_port: base_port + 2,
            hydra_odata_port: base_port + 3,
            hydra_http2_port: base_port + 4,
            hydra_mcp_port: base_port + 5,
            writer_lock_paths,
        }
    }

    fn launcher_name(&self) -> String {
        format!("launch-{}.sh", self.runtime_binary_name)
    }

    fn all_ports(&self) -> Vec<u16> {
        vec![
            self.phx_port,
            self.hydra_tcp_port,
            self.hydra_http_port,
            self.hydra_odata_port,
            self.hydra_http2_port,
            self.hydra_mcp_port,
        ]
    }
}

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

fn usage() -> &'static str {
    "\
Usage:
  axonctl <command> --project-root PATH --instance-kind dev|live --role brain|indexer|all [--json]

Commands:
  start         Start runtime — contract-honest thin shim → scripts/axon → process-compose
  stop          Orchestrated instance stop (kill processes, clean locks, verify)
  preflight     Pre-launch checks (PG accessible, binaries present, env hygiene)
  status        Health check for an instance
  liveness      Full runtime_contract liveness (brain+indexer /readyz); exit 0 iff healthy
  cutover       In-place health-gated promote to a candidate manifest, with AUTO-ROLLBACK

Options:
  --project-root PATH     Axon project root directory
  --instance-kind KIND    dev or live
  --role ROLE             brain, indexer, or all
  --json                  Machine-readable JSON output
  --hard                  (stop only) Aggressive cleanup with port-based kill
  --timeout-ms N          (stop) SIGTERM grace period in ms (default 15000)
  --manifest PATH         (cutover) candidate release manifest to promote to
  --max-polls N           (cutover) health-gate polls before auto-rollback (default 30)
  --poll-interval-ms N    (cutover) wait between health polls (default 2000)
  --bin-dir PATH          (cutover) binary swap dir (default <project-root>/bin)

Note: supervise and auto-restart are retired (REQ-AXO-901735). Use process-compose.
"
}

struct GlobalArgs {
    project_root: Option<PathBuf>,
    instance_kind: Option<String>,
    role: Option<String>,
    json: bool,
    hard: bool,
    timeout_ms: u64,
    remaining: Vec<String>,
}

fn parse_global_args(raw: Vec<String>) -> Result<(String, GlobalArgs)> {
    let mut iter = raw.into_iter();
    let command = iter.next().ok_or_else(|| anyhow!("{}", usage()))?;

    let mut args = GlobalArgs {
        project_root: None,
        instance_kind: None,
        role: None,
        json: false,
        hard: false,
        timeout_ms: 15_000,
        remaining: Vec::new(),
    };

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => break,
            "--project-root" => args.project_root = iter.next().map(PathBuf::from),
            "--instance-kind" => args.instance_kind = iter.next(),
            "--role" => args.role = iter.next(),
            "--json" => args.json = true,
            "--hard" => args.hard = true,
            "--timeout-ms" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--timeout-ms requires a value"))?;
                args.timeout_ms = value
                    .parse::<u64>()
                    .context("--timeout-ms must be a positive integer")?;
            }
            "--interval-ms" | "--max-restarts" | "--grace-ms" => {
                let _ = iter.next();
            }
            "--help" | "-h" => return Err(anyhow!("{}", usage())),
            _ => args.remaining.push(arg),
        }
    }

    Ok((command, args))
}

fn require_config(args: &GlobalArgs) -> Result<InstanceConfig> {
    let project_root = args
        .project_root
        .clone()
        .ok_or_else(|| anyhow!("--project-root is required"))?;
    let instance_kind = InstanceKind::parse(
        args.instance_kind
            .as_deref()
            .ok_or_else(|| anyhow!("--instance-kind is required"))?,
    )?;
    let role = RuntimeRole::parse(
        args.role
            .as_deref()
            .ok_or_else(|| anyhow!("--role is required"))?,
    )?;
    Ok(InstanceConfig::new(project_root, instance_kind, role))
}

/// Like `require_config` but role-agnostic: the liveness probe reads only ports +
/// manifest, which depend on `instance_kind`, not `role`. Defaults role to `all`.
fn require_config_any_role(args: &GlobalArgs) -> Result<InstanceConfig> {
    let project_root = args
        .project_root
        .clone()
        .ok_or_else(|| anyhow!("--project-root is required"))?;
    let instance_kind = InstanceKind::parse(
        args.instance_kind
            .as_deref()
            .ok_or_else(|| anyhow!("--instance-kind is required"))?,
    )?;
    let role = match args.role.as_deref() {
        Some(r) => RuntimeRole::parse(r)?,
        None => RuntimeRole::All,
    };
    Ok(InstanceConfig::new(project_root, instance_kind, role))
}

// ---------------------------------------------------------------------------
// REQ-AXO-902166 (S5) / REQ-AXO-902165 (S3 health-gate) — full-runtime_contract
// liveness probe over HTTP. axonctl has no PG client (uses pg_isready); the brain
// and indexer each expose /readyz (REQ-AXO-901735), and the brain's /readyz does a
// real SELECT 1 — so HTTP probes suffice. Reuses the reconciler's pure liveness gates.
// ---------------------------------------------------------------------------

/// GET `url`, true iff HTTP 2xx within `timeout_s`. Bounded so a hung endpoint never
/// blocks the caller (curl -sf fails on 4xx/5xx; --max-time bounds the wait).
fn http_ready(url: &str, timeout_s: u64) -> bool {
    Command::new("curl")
        .args(["-sf", "--max-time", &timeout_s.to_string(), url])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The indexer's dedicated health-probe port (REQ-AXO-901735). Env override, else the
/// per-instance default (44130 live / 44149 dev), mirroring indexer_health_http.rs.
fn indexer_health_port(kind: InstanceKind) -> u16 {
    std::env::var("AXON_INDEXER_HEALTH_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(match kind {
            InstanceKind::Live => 44130,
            InstanceKind::Dev => 44149,
        })
}

/// Pure: does a current.json body's `runtime_contract` name a separate indexer?
/// Unreadable/absent → false (gate on the brain only — safe default). Unit-testable.
fn runtime_contract_has_indexer(current_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(current_json)
        .ok()
        .and_then(|v| {
            v.get("runtime_contract")
                .and_then(|c| c.as_str())
                .map(|c| c.contains("indexer"))
        })
        .unwrap_or(false)
}

/// Does the promoted manifest's `runtime_contract` name a separate indexer? Reads
/// current.json next to the instance's graph_v2 dir.
fn manifest_indexer_expected(config: &InstanceConfig) -> bool {
    let Some(path) = config
        .db_root
        .parent()
        .map(|d| d.join("live-release/current.json"))
    else {
        return false;
    };
    match fs::read_to_string(&path) {
        Ok(text) => runtime_contract_has_indexer(&text),
        Err(_) => false,
    }
}

/// REQ-AXO-902166 (S5) — `axonctl liveness`: probe the FULL runtime_contract via HTTP
/// /readyz (brain + indexer per the manifest) and report the liveness verdict, reusing
/// the reconciler's pure gates. Exit 0 iff healthy — so a promote can gate its qualify
/// step (never qualify a half-started runtime) and the cutover FSM can decide
/// finalize-vs-auto-rollback on this verdict.
fn cmd_liveness(config: InstanceConfig, json: bool) -> Result<()> {
    let brain_url = format!("http://127.0.0.1:{}/readyz", config.hydra_http_port);
    let indexer_url = format!(
        "http://127.0.0.1:{}/readyz",
        indexer_health_port(config.instance_kind)
    );
    let indexer_expected = manifest_indexer_expected(&config);
    let brain_serving = http_ready(&brain_url, 3);
    let indexer_ready = http_ready(&indexer_url, 3);
    let l = LivenessFacts {
        brain_serving,
        indexer_expected,
        indexer_ready,
        indexer_lifecycle: if indexer_ready { "healthy" } else { "crashed_or_abandoned" }.to_string(),
        indexer_source: if indexer_ready { "http_readyz" } else { "http_readyz_down" }.to_string(),
    };
    let gates = evaluate_liveness_gates(&l);
    let all_pass = gates.iter().all(|g| g.pass);
    let phase = liveness_phase(&l).unwrap_or("healthy");
    if json {
        println!(
            "{}",
            serde_json::json!({
                "healthy": all_pass,
                "phase": phase,
                "gates": gates.iter().map(|g| serde_json::json!({"name": g.name, "pass": g.pass, "detail": g.detail})).collect::<Vec<_>>(),
                "next_action": liveness_next_action(&l),
                "probed": {"brain": brain_url, "indexer": indexer_url, "indexer_expected": indexer_expected},
            })
        );
    } else {
        for g in &gates {
            eprintln!("  [{}] {} — {}", if g.pass { "ok" } else { "FAIL" }, g.name, g.detail);
        }
        eprintln!("liveness: {}", if all_pass { "healthy" } else { phase });
    }
    if all_pass {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// REQ-AXO-902165 (MIL-AXO-051 S3) — `axonctl cutover`: the I/O executor for an
// in-place, health-gated promote with AUTO-ROLLBACK. Composes the reconciler's
// tested choreography (`drive_cutover` + `CutoverIo`) with real manifest/bin I/O
// faithful to promote_live.sh + rollback_live.sh, and the `axonctl liveness` HTTP
// health probe. The bin-swap is IN-PLACE (writer-guard exclusivity forbids two
// coexisting writers — DEC-AXO-901666), so a failed candidate becomes a blip +
// auto-recovery (restore old bin/* from current.json + restart), never the s94
// stranded outage. The file steps are free functions (tempdir-testable); only the
// process stop/start is untestable-without-a-runtime and gated on an E2E DEV
// fault-injection run before this drives a live promote.
// ---------------------------------------------------------------------------

/// `.axon(-dev)/live-release` for the instance — holds current.json / pending.json / history.
fn cutover_release_dir(config: &InstanceConfig) -> PathBuf {
    config
        .db_root
        .parent()
        .map(|d| d.join("live-release"))
        .unwrap_or_else(|| config.project_root.join(".axon/live-release"))
}

fn read_json_file(path: &Path) -> Result<serde_json::Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Write a manifest as pretty JSON + trailing newline. Keys are NOT sorted (unlike the
/// python writer's `sort_keys`); manifests are consumed field-wise, never byte-compared.
fn write_json_file(path: &Path, v: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut s = serde_json::to_string_pretty(v).context("serialize manifest")?;
    s.push('\n');
    fs::write(path, &s).with_context(|| format!("write {}", path.display()))
}

/// The `(name, entry)` artifact list of a manifest, falling back to the single primary
/// `artifact` when the `artifacts` map is absent/empty (older manifests) — same shape as
/// the python `{"axon-core": manifest["artifact"]}` fallback in promote/rollback.
fn manifest_artifact_entries(manifest: &serde_json::Value) -> Result<Vec<(String, serde_json::Value)>> {
    if let Some(map) = manifest.get("artifacts").and_then(|a| a.as_object()).filter(|m| !m.is_empty()) {
        return Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    }
    let primary = manifest
        .get("artifact")
        .cloned()
        .ok_or_else(|| anyhow!("manifest has neither a non-empty artifacts{{}} nor an artifact{{}}"))?;
    let name = primary
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("axon-brain")
        .to_string();
    Ok(vec![(name, primary)])
}

/// Verify every artifact source referenced by `manifest` exists on disk NOW, so a later
/// bin-swap (stage or rollback) cannot fail on a missing source. Fail-fast guard.
fn verify_manifest_artifacts_present(manifest: &serde_json::Value) -> Result<()> {
    let entries = manifest_artifact_entries(manifest)?;
    for (name, entry) in entries {
        let source = entry
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| anyhow!("artifact {name} has no path"))?;
        if !Path::new(source).exists() {
            return Err(anyhow!("artifact source missing: {source}"));
        }
    }
    Ok(())
}

/// Copy each `manifest.artifacts[name].path` (a sha-keyed archived binary) into
/// `bin_dir/name`, plus its build-info. The single bin-swap primitive shared by stage
/// (candidate→bin) and rollback (current→bin), mirroring rollback_live.sh:171-190.
fn copy_manifest_artifacts_to_bin(manifest: &serde_json::Value, bin_dir: &Path) -> Result<Vec<String>> {
    fs::create_dir_all(bin_dir).ok();
    let mut copied = Vec::new();
    for (name, entry) in manifest_artifact_entries(manifest)? {
        let source = entry
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| anyhow!("artifact {name} has no path"))?;
        let source = Path::new(source);
        if !source.exists() {
            return Err(anyhow!("artifact source missing: {}", source.display()));
        }
        let target = bin_dir.join(&name);
        fs::copy(source, &target)
            .with_context(|| format!("copy {} -> {}", source.display(), target.display()))?;
        // build-info alongside (best-effort — matches promote_live.sh's paired swap).
        if let Some(bi) = entry.get("build_info_path").and_then(|b| b.as_str()) {
            let bi = Path::new(bi);
            if bi.exists() {
                let _ = fs::copy(bi, bin_dir.join(format!("{name}.build-info")));
            }
        }
        copied.push(name);
    }
    if copied.is_empty() {
        return Err(anyhow!("no artifacts copied from manifest"));
    }
    Ok(copied)
}

/// snapshot: assert the current release is a valid rollback target (current.json parses
/// and its artifacts exist). Runs before any mutation. Returns the parsed manifest.
fn cutover_snapshot(release_dir: &Path) -> Result<serde_json::Value> {
    let current = release_dir.join("current.json");
    if !current.exists() {
        return Err(anyhow!(
            "no current.json at {} — cannot cutover without a rollback target (run an initial promote first)",
            current.display()
        ));
    }
    let manifest = read_json_file(&current)?;
    verify_manifest_artifacts_present(&manifest)
        .context("current.json artifacts unusable as a rollback target")?;
    Ok(manifest)
}

/// stage: write pending.json (state=staged) from the candidate manifest + swap the
/// candidate binaries into `bin_dir`. current.json is left untouched (the rollback source).
fn cutover_stage_files(candidate_manifest_path: &Path, release_dir: &Path, bin_dir: &Path) -> Result<()> {
    let mut candidate = read_json_file(candidate_manifest_path)?;
    verify_manifest_artifacts_present(&candidate).context("candidate manifest artifacts missing")?;
    candidate["state"] = serde_json::json!("staged");
    write_json_file(&release_dir.join("pending.json"), &candidate)?;
    copy_manifest_artifacts_to_bin(&candidate, bin_dir)?;
    Ok(())
}

/// finalize: archive current→history/<gen>.json, promote pending→current (state=promoted),
/// unlink pending. Mirrors promote_live.sh:471-492.
fn cutover_finalize_files(release_dir: &Path) -> Result<()> {
    let current_path = release_dir.join("current.json");
    let pending_path = release_dir.join("pending.json");
    let history = release_dir.join("history");
    fs::create_dir_all(&history).ok();
    if current_path.exists() {
        let prev = read_json_file(&current_path)?;
        let prev_gen = prev
            .get("runtime_version")
            .and_then(|r| r.get("install_generation"))
            .and_then(|g| g.as_str())
            .unwrap_or("previous")
            .to_string();
        write_json_file(&history.join(format!("{prev_gen}.json")), &prev)?;
    }
    let mut pending = read_json_file(&pending_path).context("finalize needs a pending.json")?;
    pending["state"] = serde_json::json!("promoted");
    let gen = pending
        .get("runtime_version")
        .and_then(|r| r.get("install_generation"))
        .and_then(|g| g.as_str())
        .unwrap_or("promoted")
        .to_string();
    write_json_file(&current_path, &pending)?;
    write_json_file(&history.join(format!("{gen}.json")), &pending)?;
    fs::remove_file(&pending_path).ok();
    Ok(())
}

/// rollback: restore bin/* from current.json (the untouched old manifest) and drop the
/// failed pending staging, leaving bin/* ↔ current.json coherent. Mirrors
/// promote_live.sh `rollback_bin_to_current` + pending cleanup.
fn cutover_rollback_files(release_dir: &Path, bin_dir: &Path) -> Result<()> {
    let current = read_json_file(&release_dir.join("current.json")).context("rollback needs current.json")?;
    copy_manifest_artifacts_to_bin(&current, bin_dir)?;
    fs::remove_file(release_dir.join("pending.json")).ok();
    Ok(())
}

/// The real `CutoverIo`: file steps via the free functions above; process steps (stop /
/// start) via the existing teardown helpers + a spawned `scripts/axon start full`.
struct RealCutoverIo {
    config: InstanceConfig, // role is unused here; brain+indexer are handled explicitly.
    candidate_manifest: PathBuf,
    release_dir: PathBuf,
    bin_dir: PathBuf,
    stop_timeout_ms: u64,
}

impl RealCutoverIo {
    fn role_config(&self, role: RuntimeRole) -> InstanceConfig {
        InstanceConfig::new(self.config.project_root.clone(), self.config.instance_kind, role)
    }

    /// Stop brain+indexer and verify the canonical listeners are gone (the writer guard
    /// MUST be free before a new brain can boot — runtime_boot.rs exclusivity).
    fn stop_instance(&self) -> Result<()> {
        for role in [RuntimeRole::Brain, RuntimeRole::Indexer] {
            let _ = cmd_stop(self.role_config(role), true, self.stop_timeout_ms, false);
        }
        let remaining: Vec<i32> = [RuntimeRole::Brain, RuntimeRole::Indexer]
            .iter()
            .flat_map(|r| find_instance_all_pids(&self.role_config(*r)))
            .collect();
        if remaining.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("instance not fully stopped (writer guard still held): {remaining:?}"))
        }
    }

    /// Spawn `scripts/axon --instance <kind> start full` detached (process-compose
    /// backgrounds the DAG); the health-poll gates readiness, not this spawn.
    fn spawn_start(&self) -> Result<()> {
        let axon_entry = self.config.project_root.join("scripts").join("axon");
        if !axon_entry.exists() {
            return Err(anyhow!("scripts/axon not found at {}", axon_entry.display()));
        }
        let mut cmd = Command::new("bash");
        cmd.arg(&axon_entry)
            .args(["--instance", self.config.instance_kind.label(), "start", "full"]);
        if let Ok(log) = fs::File::create(self.release_dir.join("cutover-start.log")) {
            if let Ok(errlog) = log.try_clone() {
                cmd.stdout(std::process::Stdio::from(log));
                cmd.stderr(std::process::Stdio::from(errlog));
            }
        }
        cmd.spawn().map(|_| ()).context("spawn scripts/axon start full")
    }
}

impl CutoverIo for RealCutoverIo {
    fn snapshot_current(&mut self) -> Result<(), String> {
        cutover_snapshot(&self.release_dir).map(|_| ()).map_err(|e| format!("{e:#}"))
    }
    fn stage_candidate(&mut self) -> Result<(), String> {
        cutover_stage_files(&self.candidate_manifest, &self.release_dir, &self.bin_dir)
            .map_err(|e| format!("{e:#}"))
    }
    fn restart_runtime(&mut self) -> Result<(), String> {
        self.stop_instance().map_err(|e| format!("{e:#}"))?;
        self.spawn_start().map_err(|e| format!("{e:#}"))
    }
    fn finalize(&mut self) -> Result<(), String> {
        cutover_finalize_files(&self.release_dir).map_err(|e| format!("{e:#}"))
    }
    fn rollback(&mut self) -> Result<(), String> {
        // Restore old bin/* + drop pending, then restart the OLD release so it serves again.
        cutover_rollback_files(&self.release_dir, &self.bin_dir).map_err(|e| format!("{e:#}"))?;
        self.stop_instance().map_err(|e| format!("{e:#}"))?;
        self.spawn_start().map_err(|e| format!("{e:#}"))
    }
}

/// Local flags for `cutover`, pulled from the passthrough `remaining` args.
struct CutoverArgs {
    manifest: PathBuf,
    max_polls: usize,
    poll_interval_ms: u64,
    bin_dir: Option<PathBuf>,
}

fn parse_cutover_args(remaining: &[String], project_root: &Path) -> Result<CutoverArgs> {
    let mut manifest: Option<PathBuf> = None;
    let mut max_polls = 30usize;
    let mut poll_interval_ms = 2_000u64;
    let mut bin_dir: Option<PathBuf> = None;
    let mut it = remaining.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--manifest" => manifest = it.next().map(PathBuf::from),
            "--max-polls" => {
                max_polls = it
                    .next()
                    .ok_or_else(|| anyhow!("--max-polls requires a value"))?
                    .parse()
                    .context("--max-polls must be a positive integer")?;
            }
            "--poll-interval-ms" => {
                poll_interval_ms = it
                    .next()
                    .ok_or_else(|| anyhow!("--poll-interval-ms requires a value"))?
                    .parse()
                    .context("--poll-interval-ms must be a positive integer")?;
            }
            "--bin-dir" => bin_dir = it.next().map(PathBuf::from),
            other => return Err(anyhow!("unknown cutover flag: {other}")),
        }
    }
    let manifest = manifest.ok_or_else(|| {
        anyhow!("cutover requires --manifest <candidate.json> (the release to cut over to)")
    })?;
    let _ = project_root;
    Ok(CutoverArgs {
        manifest,
        max_polls,
        poll_interval_ms,
        bin_dir,
    })
}

fn cmd_cutover(config: InstanceConfig, remaining: &[String], json: bool) -> Result<()> {
    let cargs = parse_cutover_args(remaining, &config.project_root)?;
    let release_dir = cutover_release_dir(&config);
    let bin_dir = cargs
        .bin_dir
        .unwrap_or_else(|| config.project_root.join("bin"));

    // Health probe = the FULL runtime_contract liveness (same gates as `axonctl liveness`).
    let brain_url = format!("http://127.0.0.1:{}/readyz", config.hydra_http_port);
    let indexer_url = format!(
        "http://127.0.0.1:{}/readyz",
        indexer_health_port(config.instance_kind)
    );
    let indexer_expected = manifest_indexer_expected(&config);
    let probe = || {
        let brain_ok = http_ready(&brain_url, 3);
        let indexer_ok = !indexer_expected || http_ready(&indexer_url, 3);
        brain_ok && indexer_ok
    };
    let wait = || thread::sleep(Duration::from_millis(cargs.poll_interval_ms));

    let mut io = RealCutoverIo {
        candidate_manifest: cargs.manifest.clone(),
        release_dir: release_dir.clone(),
        bin_dir,
        stop_timeout_ms: 15_000,
        config,
    };

    let verdict = drive_cutover(&mut io, probe, cargs.max_polls, wait);

    let (ok, phase, detail): (bool, &str, Option<String>) = match &verdict {
        CutoverVerdict::Promoted => (true, "promoted", None),
        CutoverVerdict::RolledBack {
            failed_step,
            rollback_ok,
            detail,
        } => (
            false,
            if *rollback_ok { "rolled_back" } else { "rollback_failed" },
            Some(format!(
                "failed at {failed_step}{}{}",
                detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default(),
                if *rollback_ok {
                    " — old release restored & restarting"
                } else {
                    " — ROLLBACK ALSO FAILED, operator action required"
                }
            )),
        ),
    };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "cutover",
                "instance": io.config.instance_kind.label(),
                "candidate_manifest": cargs.manifest.display().to_string(),
                "promoted": ok,
                "phase": phase,
                "detail": detail,
                "max_polls": cargs.max_polls,
            })
        );
    } else if ok {
        println!(
            "axonctl cutover: {} promoted to {} ✓",
            io.config.instance_kind.label(),
            cargs.manifest.display()
        );
    } else {
        eprintln!(
            "axonctl cutover: {} NOT promoted (phase={}) — {}",
            io.config.instance_kind.label(),
            phase,
            detail.as_deref().unwrap_or("")
        );
    }

    if ok {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn main() -> Result<()> {
    let all_args: Vec<String> = std::env::args().skip(1).collect();
    let (command, args) = parse_global_args(all_args)?;

    match command.as_str() {
        "stop" => {
            let base = require_config(&args)?;
            let roles = base.role.concrete_roles();
            let mut result = Ok(());
            for role in roles {
                let cfg = InstanceConfig::new(base.project_root.clone(), base.instance_kind, role);
                if let Err(e) = cmd_stop(cfg, args.hard, args.timeout_ms, args.json) {
                    result = Err(e);
                }
            }
            result
        }
        "start" => cmd_start(require_config(&args)?, &args.remaining, args.json),
        "supervise" => {
            eprintln!("axonctl supervise is retired — use process-compose (REQ-AXO-901735)");
            std::process::exit(1);
        }
        "preflight" => cmd_preflight(require_config(&args)?, args.json),
        "auto-restart" => {
            eprintln!("axonctl auto-restart is retired — use process-compose (REQ-AXO-901735)");
            std::process::exit(1);
        }
        "status" => {
            let base = require_config(&args)?;
            let roles = base.role.concrete_roles();
            for role in roles {
                let cfg = InstanceConfig::new(base.project_root.clone(), base.instance_kind, role);
                cmd_status(cfg, args.json)?;
            }
            Ok(())
        }
        // REQ-AXO-902166 (S5) — full-runtime_contract liveness probe (HTTP /readyz).
        "liveness" => cmd_liveness(require_config_any_role(&args)?, args.json),
        // REQ-AXO-902165 (S3) — in-place health-gated cutover + auto-rollback.
        "cutover" => cmd_cutover(require_config_any_role(&args)?, &args.remaining, args.json),
        "help" | "--help" | "-h" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(anyhow!("unknown command `{other}`\n{}", usage())),
    }
}

// ---------------------------------------------------------------------------
// REQ-AXO-901847 (DEC-AXO-901651): axonctl start — contract-honest thin shim.
// axonctl does NOT re-absorb the process-compose orchestration (that decision
// was settled in REQ-AXO-901735: supervision retired). Instead `axonctl start`
// validates the instance config and execs the canonical `scripts/axon` entry,
// which sets up devenv/env/ports (LD_LIBRARY_PATH/ORT — the fragile WSL2/CUDA
// bits) and execs start.sh (the process-compose DAG executor). This gives
// axonctl a complete verb surface (start/stop/status/preflight) with zero
// duplication of the shell env logic.
// ---------------------------------------------------------------------------

/// Build the argv for the start shim: delegate to `scripts/axon --instance
/// <kind> start <extra...>`. Factored out so the contract is unit-testable
/// without execing.
fn start_argv(axon_entry: &Path, instance_label: &str, extra: &[String]) -> Vec<String> {
    let mut argv = vec![
        "bash".to_string(),
        axon_entry.display().to_string(),
        "--instance".to_string(),
        instance_label.to_string(),
        "start".to_string(),
    ];
    argv.extend(extra.iter().cloned());
    argv
}

fn cmd_start(config: InstanceConfig, extra: &[String], json: bool) -> Result<()> {
    let axon_entry = config.project_root.join("scripts").join("axon");
    if !axon_entry.exists() {
        return Err(anyhow!(
            "canonical entry not found: {} (axonctl start delegates to scripts/axon)",
            axon_entry.display()
        ));
    }
    let argv = start_argv(&axon_entry, config.instance_kind.label(), extra);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "action": "start",
                "instance": config.instance_kind.label(),
                "delegates_to": axon_entry.display().to_string(),
                "argv": argv,
            })
        );
    }
    // Replace this process so the operator's signals and the child's exit code
    // pass through transparently (mirrors scripts/axon's `exec bash start.sh`).
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(anyhow!("failed to exec {}: {err}", axon_entry.display()))
}

// ---------------------------------------------------------------------------
// Phase 1: axonctl stop — orchestrated instance stop
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StopReport {
    instance_kind: String,
    role: String,
    phases: Vec<StopPhase>,
    remaining_pids: Vec<i32>,
    status: String,
    /// Stop-FSM verdict (REQ-AXO-902111): derived projection of the teardown state.
    /// `phase` ∈ stopping|stopped|orphaned|partial; `failed_gates` lists the gate
    /// names that did NOT pass; `next_action` is the single corrective step (or null
    /// when the stop reached a terminal good state / is merely draining).
    phase: String,
    failed_gates: Vec<String>,
    next_action: Option<String>,
}

#[derive(Debug, Serialize)]
struct StopPhase {
    name: String,
    pids_targeted: Vec<i32>,
    detail: String,
}

fn cmd_stop(config: InstanceConfig, hard: bool, timeout_ms: u64, json: bool) -> Result<()> {
    let mut phases = Vec::new();
    let mut all_killed = Vec::new();

    // Steps 1-5: Independent discovery+kill phases run in parallel.
    // Each thread discovers targets, sends SIGTERM, and returns its phase report.
    let (tracked, beam, tree, tmux, ports) = std::thread::scope(|s| {
        // Step 1: Kill tracked PID from pid file
        let t_tracked = s.spawn(|| -> (Vec<i32>, Option<StopPhase>) {
            if let Ok(Some(pid)) = read_pid_file(&config.pid_file) {
                if process_exists(pid) && process_cmdline_matches_instance(pid, &config) {
                    terminate_pids(&[pid], libc::SIGTERM);
                    return (
                        vec![pid],
                        Some(StopPhase {
                            name: "kill_tracked_pid".into(),
                            pids_targeted: vec![pid],
                            detail: format!(
                                "SIGTERM to tracked PID from {}",
                                config.pid_file.display()
                            ),
                        }),
                    );
                }
            }
            (vec![], None)
        });

        // Step 2: Kill BEAM processes by Erlang node name
        let t_beam = s.spawn(|| -> (Vec<i32>, Option<StopPhase>) {
            let beam_pids = find_beam_pids_by_node_name(&config.elixir_node_name);
            if !beam_pids.is_empty() {
                terminate_pids(&beam_pids, libc::SIGTERM);
                let phase = StopPhase {
                    name: "kill_beam_by_node_name".into(),
                    pids_targeted: beam_pids.clone(),
                    detail: format!("BEAM processes matching node {}", config.elixir_node_name),
                };
                return (beam_pids, Some(phase));
            }
            (vec![], None)
        });

        // Step 3: Kill process tree (runtime + launcher + descendants)
        let t_tree = s.spawn(|| -> (Vec<i32>, Option<StopPhase>) {
            let tree_pids = find_instance_process_tree(&config);
            if !tree_pids.is_empty() {
                terminate_pids(&tree_pids, libc::SIGTERM);
                let phase = StopPhase {
                    name: "kill_process_tree".into(),
                    pids_targeted: tree_pids.clone(),
                    detail: "Matching runtime/launcher processes and descendants".into(),
                };
                return (tree_pids, Some(phase));
            }
            (vec![], None)
        });

        // Step 4: Kill tmux session
        let t_tmux = s.spawn(|| -> (Vec<i32>, Option<StopPhase>) {
            let killed = kill_tmux_session(&config.tmux_session);
            if killed {
                return (
                    vec![],
                    Some(StopPhase {
                        name: "kill_tmux_session".into(),
                        pids_targeted: vec![],
                        detail: format!("tmux kill-session -t {}", config.tmux_session),
                    }),
                );
            }
            (vec![], None)
        });

        // Step 5: Kill port listeners (hard mode only)
        let t_ports = s.spawn(|| -> (Vec<i32>, Option<StopPhase>) {
            if !hard {
                return (vec![], None);
            }
            let port_pids = find_port_listener_pids(&config);
            if !port_pids.is_empty() {
                terminate_pids(&port_pids, libc::SIGTERM);
                let phase = StopPhase {
                    name: "kill_port_listeners".into(),
                    pids_targeted: port_pids.clone(),
                    detail: format!("Port listeners on {:?}", config.all_ports()),
                };
                return (port_pids, Some(phase));
            }
            (vec![], None)
        });

        (
            t_tracked.join().unwrap(),
            t_beam.join().unwrap(),
            t_tree.join().unwrap(),
            t_tmux.join().unwrap(),
            t_ports.join().unwrap(),
        )
    });

    // Collect phases and PIDs in deterministic order (same as the old sequential flow).
    for (pids, phase) in [tracked, beam, tree, tmux, ports] {
        all_killed.extend(pids);
        if let Some(p) = phase {
            phases.push(p);
        }
    }

    // Deduplicate PIDs (threads may discover overlapping sets).
    all_killed.sort_unstable();
    all_killed.dedup();

    // Wait for all SIGTERM-ed processes to exit
    wait_for_gone(&all_killed, Duration::from_millis(timeout_ms));

    // Escalate: SIGKILL remaining
    let remaining = live_pids(&all_killed);
    if !remaining.is_empty() {
        terminate_pids(&remaining, libc::SIGKILL);
        wait_for_gone(&remaining, Duration::from_millis(2_000));
        phases.push(StopPhase {
            name: "sigkill_escalation".into(),
            pids_targeted: remaining,
            detail: "SIGKILL after SIGTERM timeout".into(),
        });
    }

    // Step 6: Cleanup stale writer locks
    let lock_cleaned = cleanup_stale_locks(&config);
    if !lock_cleaned.is_empty() {
        phases.push(StopPhase {
            name: "cleanup_stale_locks".into(),
            pids_targeted: vec![],
            detail: format!("Removed stale locks: {}", lock_cleaned.join(", ")),
        });
    }

    // Step 7: Cleanup sockets and PID file
    cleanup_files(&[&config.telemetry_sock, &config.mcp_sock, &config.pid_file]);

    // Step 8: Final verification
    let final_remaining = find_instance_all_pids(&config);

    // Step 9: Stop-FSM verdict (REQ-AXO-902111). Project the teardown facts already
    // computed above onto the declarative stop gates. We do NOT change the kill/cleanup
    // logic — we only read the residual state and derive {phase, failed_gates,
    // next_action} so an LLM/operator reads a one-line verdict instead of re-deriving it.
    let stop_facts = StopFacts {
        // Role we were asked to stop ("all" | "brain" | "indexer").
        stop_role: config.role.label().to_string(),
        // Survivors of the teardown = the canonical listeners still alive.
        canonical_listeners: final_remaining.clone(),
        // Any of this instance's canonical ports still in LISTEN (brain MCP included).
        brain_port_bound: !find_port_listener_pids(&config).is_empty(),
        // The process-compose supervisor for this instance is still alive.
        supervisor_healthy: probe_supervisor_healthy(&config),
        // Writer lock files still on disk (cleanup_stale_locks only reaps dead-owner
        // locks; a lock held by a live survivor remains and is a real residual).
        writer_locks_held: config
            .writer_lock_paths
            .iter()
            .filter(|(_, path)| path.exists())
            .map(|(target, _)| target.clone())
            .collect(),
        // Control sockets still present after the step-7 unlink (best-effort).
        sockets_present: config.telemetry_sock.exists() || config.mcp_sock.exists(),
        // Indexer heartbeat freshness is owned by the brain's in-process status source,
        // not reachable from axonctl — best-effort false (never a false "draining").
        indexer_heartbeat_fresh: false,
    };
    let stop_gates = evaluate_stop_gates(&stop_facts);
    let failed_gates: Vec<String> = stop_gates
        .iter()
        .filter(|g| !g.pass)
        .map(|g| g.name.to_string())
        .collect();
    let stop_phase_verdict = stop_phase(&stop_facts).to_string();
    let stop_next = stop_next_action(&stop_facts);

    let report = StopReport {
        instance_kind: config.instance_kind.label().to_string(),
        role: config.role.label().to_string(),
        phases,
        status: if final_remaining.is_empty() {
            "stopped".to_string()
        } else {
            "remaining".to_string()
        },
        remaining_pids: final_remaining.clone(),
        phase: stop_phase_verdict,
        failed_gates,
        next_action: stop_next,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if final_remaining.is_empty() {
            println!(
                "axonctl stop: {} {} stopped cleanly (phase={})",
                report.instance_kind, report.role, report.phase
            );
        } else {
            eprintln!(
                "axonctl stop: {} {} has remaining pids: {:?} (phase={})",
                report.instance_kind, report.role, final_remaining, report.phase
            );
        }
        if let Some(action) = &report.next_action {
            eprintln!("axonctl stop: next action — {action}");
        }
    }

    if final_remaining.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("processes still alive: {:?}", final_remaining))
    }
}

/// Check if a process belongs to the OPPOSITE instance (dev vs live) sharing
/// the same project_root. A match here means we must NOT kill this process.
///
/// Detection strategies (in priority order):
/// 1. Supervisor cmdline contains `--instance-kind <opposite>`
/// 2. Process was launched from the opposite instance directory
///    (e.g., `.axon-dev/` for dev, `bin/` or `.axon/` for live)
fn cmdline_belongs_to_opposite_instance(cmdline: &str, config: &InstanceConfig) -> bool {
    let opposite = config.instance_kind.opposite_label();
    // Strategy 1: axonctl supervise --instance-kind <opposite> in cmdline
    if cmdline.contains(&format!("--instance-kind\0{opposite}"))
        || cmdline.contains(&format!("--instance-kind {opposite}"))
    {
        return true;
    }
    // Strategy 2: instance-specific directory markers in cmdline path
    match config.instance_kind {
        InstanceKind::Dev => {
            // We are stopping dev; if cmdline shows the live release binary
            // path (bin/axon-*) without any dev marker, it's live.
            let root = config.project_root.to_string_lossy();
            if cmdline.contains(&format!("{root}/bin/"))
                && !cmdline.contains(".axon-dev/")
                && !cmdline.contains("--instance-kind\0dev")
                && !cmdline.contains("--instance-kind dev")
            {
                return true;
            }
        }
        InstanceKind::Live => {
            // We are stopping live; if cmdline shows .axon-dev/ path, it's dev.
            if cmdline.contains(".axon-dev/") {
                return true;
            }
        }
    }
    false
}

fn process_cmdline_matches_instance(pid: i32, config: &InstanceConfig) -> bool {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let Ok(raw) = fs::read(&cmdline_path) else {
        return false;
    };
    let cmdline = String::from_utf8_lossy(&raw);

    // SAFETY: never kill a process belonging to the opposite instance
    if cmdline_belongs_to_opposite_instance(&cmdline, config) {
        return false;
    }

    let root = config.project_root.to_string_lossy();
    // Match by project_root + binary name (absolute path launch)
    if cmdline.contains(root.as_ref())
        && (cmdline.contains(&config.runtime_binary_name)
            || cmdline.contains(&config.launcher_name()))
    {
        return true;
    }
    // Match by binary name alone when launched with relative path from project root
    // (e.g., ".axon/cargo-target/debug/axon-indexer")
    if cmdline.contains(&config.runtime_binary_name) {
        // Verify the process cwd is under project_root
        let cwd_link = format!("/proc/{pid}/cwd");
        if let Ok(cwd) = fs::read_link(&cwd_link) {
            return cwd.starts_with(&config.project_root);
        }
    }
    false
}

fn process_matches_instance(entry: &ProcEntry, config: &InstanceConfig) -> bool {
    // SAFETY: never match a process belonging to the opposite instance
    if cmdline_belongs_to_opposite_instance(&entry.command, config) {
        return false;
    }

    let root = config.project_root.to_string_lossy();
    // Match runtime binary or launcher within project root
    if entry.command.contains(root.as_ref())
        && (entry.command.contains(&config.runtime_binary_name)
            || entry.command.contains(&config.launcher_name()))
    {
        return true;
    }
    // Match BEAM by Erlang node name (orphaned BEAMs may lack project_root in cmdline)
    if entry.command.contains("beam.smp") && entry.command.contains(&config.elixir_node_name) {
        return true;
    }
    // Match dashboard build tools under project root
    if entry.command.contains(root.as_ref())
        && (entry.command.contains("_build/esbuild") || entry.command.contains("_build/tailwind"))
    {
        return true;
    }
    false
}

fn find_beam_pids_by_node_name(node_name: &str) -> Vec<i32> {
    proc_entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.command.contains("beam.smp") && e.command.contains(node_name))
        .map(|e| e.pid)
        .collect()
}

fn find_instance_process_tree(config: &InstanceConfig) -> Vec<i32> {
    let entries = proc_entries().unwrap_or_default();
    let mut pids = BTreeSet::new();
    for entry in &entries {
        if process_matches_instance(entry, config) {
            pids.insert(entry.pid);
            if let Ok(descendants) = descendant_tree(entry.pid) {
                pids.extend(descendants);
            }
        }
    }
    pids.into_iter().collect()
}

fn find_instance_all_pids(config: &InstanceConfig) -> Vec<i32> {
    proc_entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| process_matches_instance(e, config))
        .map(|e| e.pid)
        .collect()
}

fn find_port_listener_pids(config: &InstanceConfig) -> Vec<i32> {
    let output = Command::new("ss").args(["-ltnp"]).output().ok();
    let Some(output) = output else { return vec![] };
    if !output.status.success() {
        return vec![];
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ports = config.all_ports();
    let mut pids = Vec::new();
    for line in stdout.lines() {
        for port in &ports {
            let port_pattern = format!(":{port}");
            if line.contains(&port_pattern) {
                // Extract PID from ss output: "pid=12345,"
                if let Some(pid_start) = line.find("pid=") {
                    let after = &line[pid_start + 4..];
                    if let Some(end) = after.find(|c: char| !c.is_ascii_digit()) {
                        if let Ok(pid) = after[..end].parse::<i32>() {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// REQ-AXO-902111 — best-effort probe of the process-compose supervisor for THIS
/// instance. A live supervisor on a full teardown is an orphan (it will respawn the
/// role just killed); on a role-scoped stop it is expected. Detection: a running
/// `process-compose` process whose cmdline references this instance's project_root and
/// does NOT belong to the opposite instance (dev↔live share a project_root). Returns
/// `false` when `ps` is unavailable or no match is found (indeterminable → not healthy),
/// which is the conservative default for the stop verdict.
fn probe_supervisor_healthy(config: &InstanceConfig) -> bool {
    let root = config.project_root.to_string_lossy().to_string();
    proc_entries()
        .unwrap_or_default()
        .into_iter()
        .any(|e| {
            e.command.contains("process-compose")
                && e.command.contains(&root)
                && !cmdline_belongs_to_opposite_instance(&e.command, config)
        })
}

fn kill_tmux_session(session: &str) -> bool {
    let has = Command::new("tmux")
        .args(["has-session", "-t", session])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has {
        return false;
    }
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", session])
        .output();
    // Retry once after brief wait
    thread::sleep(Duration::from_millis(500));
    let still = Command::new("tmux")
        .args(["has-session", "-t", session])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if still {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", session])
            .output();
    }
    true
}

fn cleanup_stale_locks(config: &InstanceConfig) -> Vec<String> {
    let mut cleaned = Vec::new();
    for (target, path) in &config.writer_lock_paths {
        if !path.exists() {
            continue;
        }
        if let Some(owner_pid) = parse_lock_file_pid(path) {
            if !process_exists(owner_pid) {
                if fs::remove_file(path).is_ok() {
                    cleaned.push(format!("{target} (owner pid={owner_pid} dead)"));
                }
            }
        }
    }
    cleaned
}

fn parse_lock_file_pid(path: &Path) -> Option<i32> {
    let content = fs::read_to_string(path).ok()?;
    // Format from runtime_writer_guard.rs: "owner=identity;pid=12345"
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("owner=") {
            if let Some(pid_part) = rest.split(";pid=").nth(1) {
                return pid_part.trim().parse::<i32>().ok();
            }
        }
    }
    None
}

fn cleanup_files(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

// cmd_supervise and cmd_auto_restart retired by REQ-AXO-901735.
// Process supervision is now handled by process-compose.
// The dispatch table above prints a retirement message and exits.

// ---------------------------------------------------------------------------
// Phase 3: axonctl status — health check
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusReport {
    instance_kind: String,
    role: String,
    process: ProcessStatus,
    ports: Vec<PortStatus>,
    sockets: Vec<SocketStatus>,
    writer_guards: Vec<WriterGuardStatus>,
    /// REQ-AXO-151 — list of role contract items the runtime is missing.
    /// Examples: `mcp_unavailable` for a brain instance whose MCP surface is
    /// neither exposed via unix socket nor via HTTP port (REQ-AXO-156);
    /// `telemetry_socket_missing` for an indexer whose telemetry stream is
    /// part of its operator contract. When this list is non-empty, `overall`
    /// is downgraded to `degraded` even if the process is alive.
    role_contract_violations: Vec<String>,
    /// REQ-AXO-901879 — liveness backed by canonical PIL-AXO-001 evidence
    /// (role-port bound / writer-guard owner alive), not the pid file alone.
    /// Under the process-compose DAG the role binaries are launched directly
    /// and never write the legacy pid file, so a pid-file-only check reported a
    /// false DOWN on a healthy runtime. `liveness_source` names the signal.
    effective_alive: bool,
    liveness_source: String,
    overall: String,
}

/// REQ-AXO-151 — given the runtime role and the observed socket / port state,
/// return the role contract items the instance is failing to satisfy.
///
/// REQ-AXO-156 — MCP availability is satisfied when EITHER the unix socket
/// is present OR the MCP HTTP port is listening. Production brains may run
/// HTTP-only (no socket file), so a strict socket-presence check would
/// false-positive `mcp_socket_missing` on a fully working live runtime.
///
/// Returns an empty vec when the contract is whole. Pure function for unit
/// testing.
fn compute_role_contract_violations(
    role: RuntimeRole,
    sockets: &[SocketStatus],
    mcp_http_listening: bool,
) -> Vec<String> {
    let socket_present =
        |name: &str| -> bool { sockets.iter().any(|s| s.name == name && s.exists) };
    let mcp_available = socket_present("mcp") || mcp_http_listening;
    let mut violations = Vec::new();
    match role {
        RuntimeRole::Brain => {
            if !mcp_available {
                violations.push("mcp_unavailable".to_string());
            }
        }
        RuntimeRole::Indexer => {
            if !socket_present("telemetry") {
                violations.push("telemetry_socket_missing".to_string());
            }
        }
        RuntimeRole::All => {
            if !mcp_available {
                violations.push("mcp_unavailable".to_string());
            }
            if !socket_present("telemetry") {
                violations.push("telemetry_socket_missing".to_string());
            }
        }
    }
    violations
}

#[derive(Debug, Serialize)]
struct ProcessStatus {
    pid_file_exists: bool,
    pid: Option<i32>,
    alive: bool,
    cmdline_matches: bool,
}

#[derive(Debug, Serialize)]
struct PortStatus {
    port: u16,
    listening: bool,
}

#[derive(Debug, Serialize)]
struct SocketStatus {
    name: String,
    path: String,
    exists: bool,
}

#[derive(Debug, Serialize)]
struct WriterGuardStatus {
    target: String,
    path: String,
    exists: bool,
    owner_pid: Option<i32>,
    owner_alive: bool,
    stale: bool,
}

fn cmd_status(config: InstanceConfig, json: bool) -> Result<()> {
    // Process check
    let pid_file_exists = config.pid_file.exists();
    let pid = if pid_file_exists {
        read_pid_file(&config.pid_file).ok().flatten()
    } else {
        None
    };
    let alive = pid.map(process_exists).unwrap_or(false);
    let cmdline_matches = pid
        .filter(|_| alive)
        .map(|p| process_cmdline_matches_instance(p, &config))
        .unwrap_or(false);

    let process = ProcessStatus {
        pid_file_exists,
        pid,
        alive,
        cmdline_matches,
    };

    // Port checks
    let listening_ports = get_listening_ports();
    let ports: Vec<PortStatus> = config
        .all_ports()
        .into_iter()
        .map(|port| PortStatus {
            port,
            listening: listening_ports.contains(&port),
        })
        .collect();

    // Socket checks
    let sockets = vec![
        SocketStatus {
            name: "telemetry".into(),
            path: config.telemetry_sock.to_string_lossy().into(),
            exists: config.telemetry_sock.exists(),
        },
        SocketStatus {
            name: "mcp".into(),
            path: config.mcp_sock.to_string_lossy().into(),
            exists: config.mcp_sock.exists(),
        },
    ];

    // Writer guard checks
    let mut writer_guards = Vec::new();
    for (target, path) in &config.writer_lock_paths {
        let exists = path.exists();
        let owner_pid = if exists {
            parse_lock_file_pid(path)
        } else {
            None
        };
        let owner_alive = owner_pid.map(process_exists).unwrap_or(false);
        let stale = exists && !owner_alive;
        writer_guards.push(WriterGuardStatus {
            target: target.clone(),
            path: path.to_string_lossy().into(),
            exists,
            owner_pid,
            owner_alive,
            stale,
        });
    }

    // REQ-AXO-151 — role contract: brain MUST expose its MCP surface;
    // indexer MUST expose its telemetry socket. A live process whose role
    // contract is broken is `degraded`, never `healthy`. REQ-AXO-156 — MCP
    // availability is satisfied via socket OR `hydra_http_port` listening,
    // since production brains may serve HTTP-only.
    let mcp_http_listening = ports
        .iter()
        .any(|p| p.port == config.hydra_http_port && p.listening);
    let role_contract_violations =
        compute_role_contract_violations(config.role, &sockets, mcp_http_listening);

    // REQ-AXO-901879 — back liveness with canonical PIL-AXO-001 evidence, not
    // the pid file alone. Under the process-compose DAG the role binaries are
    // launched directly and never write the legacy run-{role} pid file, so the
    // pid-file-derived `alive` is a false DOWN on a healthy runtime. A brain
    // that serves its MCP surface, or an indexer that holds a live writer
    // guard, IS alive.
    let mcp_available = mcp_http_listening || sockets.iter().any(|s| s.name == "mcp" && s.exists);
    let guard_owner_live = writer_guards.iter().any(|g| g.exists && g.owner_alive);
    let role_liveness_signal = match config.role {
        RuntimeRole::Brain => mcp_available,
        RuntimeRole::Indexer => guard_owner_live,
        RuntimeRole::All => mcp_available || guard_owner_live,
    };
    let effective_alive = alive || role_liveness_signal;
    let liveness_source = if alive {
        "pid_file"
    } else if role_liveness_signal {
        match config.role {
            RuntimeRole::Brain => "mcp_surface",
            RuntimeRole::Indexer => "writer_guard",
            RuntimeRole::All => "mcp_or_guard",
        }
    } else {
        "none"
    };

    let overall = if !effective_alive {
        "down"
    } else if alive && !cmdline_matches {
        // pid file points to a live but non-matching process — real drift.
        "degraded"
    } else if !role_contract_violations.is_empty() {
        "degraded"
    } else {
        "healthy"
    };

    let report = StatusReport {
        instance_kind: config.instance_kind.label().to_string(),
        role: config.role.label().to_string(),
        process,
        ports,
        sockets,
        writer_guards,
        role_contract_violations,
        effective_alive,
        liveness_source: liveness_source.to_string(),
        overall: overall.to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "axonctl status: {} {} = {}",
            report.instance_kind, report.role, report.overall
        );
        if let Some(pid) = report.process.pid {
            println!(
                "  process: pid={pid} alive={} match={}",
                report.process.alive, report.process.cmdline_matches
            );
        } else if report.effective_alive {
            println!(
                "  process: live via {} (no pid file)",
                report.liveness_source
            );
        } else {
            println!("  process: no pid file");
        }
        for p in &report.ports {
            if p.listening {
                println!("  port {}: listening", p.port);
            }
        }
        for s in &report.sockets {
            if s.exists {
                println!("  socket {}: present", s.name);
            }
        }
        for g in &report.writer_guards {
            if g.exists {
                let state = if g.stale { "STALE" } else { "held" };
                println!("  guard {}: {} (pid={:?})", g.target, state, g.owner_pid);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// REQ-AXO-097 — auto-restart: cross-process restart half of the
// runtime watchdog. The in-process detection (runtime_watchdog +
// runtime_readiness staleness flipper) marks subsystems Failed when
// their tokio task dies. axonctl's auto-restart is the supervisor
// half: it polls the role process's pid + cmdline, and when the role
// is observed dead it spawns the user-supplied restart command.
// Together, this closes REQ-AXO-097 — a SIGKILLed indexer is
// detected and respawned without operator intervention.
// ---------------------------------------------------------------------------
//
fn get_listening_ports() -> BTreeSet<u16> {
    let output = Command::new("ss").args(["-ltn"]).output().ok();
    let Some(output) = output else {
        return BTreeSet::new();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = BTreeSet::new();
    for line in stdout.lines().skip(1) {
        // Format: "LISTEN 0 128 *:44137 *:*"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            if let Some(port_str) = parts[3].rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    ports.insert(port);
                }
            }
        }
    }
    ports
}

// ---------------------------------------------------------------------------
// Phase 5 (REQ-AXO-901735) : axonctl preflight — pre-launch checks
// ---------------------------------------------------------------------------
//
// V1 MVP : 2 checks suffisent à éliminer le cas root du bug 2026-05-24
// (PG mort post-reboot Windows, 94 s d'opacité avant échec générique).
// V2 raffinera (role axon existe, target DB seedée, DDL appliqué).
//
// L'output JSON est consommable par process-compose `depends_on` exec
// probe (l'exit code 0 = green light, 1 = bloquer le boot des
// consumers). Comme remplacement de scripts/lib/ensure-runtime.sh.

#[derive(Debug, Serialize)]
struct PreflightCheck {
    name: String,
    passed: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct PreflightReport {
    instance_kind: String,
    role: String,
    pg_port: u16,
    checks: Vec<PreflightCheck>,
    status: String,
}

fn run_pg_isready(port: u16) -> PreflightCheck {
    // pg_isready returns 0 if accepting, 1 if rejecting, 2 if no response,
    // 3 on invocation error. We treat 0 as passed and capture stderr.
    match Command::new("pg_isready")
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-q"])
        .output()
    {
        Ok(out) if out.status.success() => PreflightCheck {
            name: "pg_isready".to_string(),
            passed: true,
            detail: format!("PG responding on :{port}"),
        },
        Ok(out) => PreflightCheck {
            name: "pg_isready".to_string(),
            passed: false,
            detail: format!(
                "PG not ready on :{port} (rc={}): {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => PreflightCheck {
            name: "pg_isready".to_string(),
            passed: false,
            detail: format!("pg_isready binary unavailable: {e}"),
        },
    }
}

fn check_binary_present(path: &Path) -> PreflightCheck {
    let exists = path.exists();
    PreflightCheck {
        name: format!("binary:{}", path.display()),
        passed: exists,
        detail: if exists {
            format!("{} present", path.display())
        } else {
            format!("{} missing — promote_live_safe.sh required", path.display())
        },
    }
}

/// REQ-AXO-901739 — refuser les env vars retirées par migrations SOLL.
/// Instance du pattern CPT-AXO-90034 (technology migration residue tracking).
/// La liste est statique et trackée avec la SOLL ref qui l a retirée.
fn check_env_no_stale_vars() -> PreflightCheck {
    // (env_var_name, soll_ref_explaining_retirement)
    let retired: &[(&str, &str)] = &[
        ("AXON_AGE_ONLY_RELATIONS", "MIL-AXO-017 retire Apache AGE"),
        (
            "AXON_INDEXER_PG_OPT_IN",
            "MIL-AXO-017 Gate 7 merged 2026-05-13",
        ),
        ("AXON_DUCKDB_WAL_DIR", "REQ-AXO-271 slice 2-6 retire DuckDB"),
        ("AXON_DUCKDB_OPT_IN", "REQ-AXO-271 retire DuckDB"),
        (
            "AXON_HOT_STATUS_CACHE",
            "REQ-AXO-901653 slice-5d retire FVQ flush path",
        ),
        (
            "AXON_FILE_VECTORIZATION_QUEUE_DEPTH",
            "REQ-AXO-901632 retire FVQ",
        ),
        (
            "AXON_FILE_VECTORIZATION_QUEUE_TIMEOUT_MS",
            "REQ-AXO-901632 retire FVQ",
        ),
        (
            "AXON_FVQ_TELEMETRY_ENABLED",
            "REQ-AXO-901674 purge FVQ/GPQ telemetry",
        ),
        (
            "AXON_GPQ_TELEMETRY_ENABLED",
            "REQ-AXO-901674 purge FVQ/GPQ telemetry",
        ),
        (
            "AXON_QUEUE_MEMORY_BUDGET_BYTES",
            "REQ-AXO-290 S3 retire env-gated queue cap",
        ),
        (
            "AXON_GPU_EMBED_SERVICE_TENSORRT",
            "REQ-AXO-901737 retire indirection ; AXON_EMBEDDING_PROVIDER=tensorrt suffit",
        ),
        (
            "AXON_REQUEST_TENSORRT",
            "REQ-AXO-901737 retire indirection ; AXON_EMBEDDING_PROVIDER=tensorrt suffit",
        ),
        (
            "AXON_EMBEDDING_GPU_PRESENT",
            "REQ-AXO-901737 retire env-fanout ; in-process struct EmbeddingProviderDiagnostics",
        ),
        (
            "AXON_EMBEDDING_PROVIDER_EFFECTIVE",
            "REQ-AXO-901737 retire env-fanout",
        ),
        (
            "AXON_EMBEDDING_PROVIDER_INIT_ERROR",
            "REQ-AXO-901737 retire env-fanout",
        ),
    ];

    let leaks: Vec<String> = retired
        .iter()
        .filter(|(var, _)| std::env::var(var).is_ok())
        .map(|(var, soll)| format!("  {} → retiré par {}", var, soll))
        .collect();

    if leaks.is_empty() {
        PreflightCheck {
            name: "env-stale-vars".to_string(),
            passed: true,
            detail: format!("Aucune des {} env vars retirées présente", retired.len()),
        }
    } else {
        PreflightCheck {
            name: "env-stale-vars".to_string(),
            passed: false,
            detail: format!(
                "{} env var(s) retirée(s) détectée(s) — résidu de session précédente, peut causer comportement indéfini :\n{}",
                leaks.len(),
                leaks.join("\n")
            ),
        }
    }
}

/// REQ-AXO-901739 — `ORT_DYLIB_PATH` doit pointer vers l artifact TensorRT
/// canonical, pas vers l onnxruntime nixpkgs default (la trap REQ-AXO-901630).
/// V1 : reject si le chemin contient `onnxruntime` MAIS PAS `tensorrt` ;
/// reject si le chemin n existe pas sur disque. V2 raffinera contre le
/// manifest .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json.
fn check_ort_dylib_path_canonical() -> PreflightCheck {
    match std::env::var("ORT_DYLIB_PATH") {
        Err(_) => PreflightCheck {
            name: "ort-dylib-path".to_string(),
            passed: true,
            detail:
                "ORT_DYLIB_PATH unset — runtime résoudra via artifact manifest (REQ-AXO-901630)"
                    .to_string(),
        },
        Ok(path) => {
            let trimmed = path.trim().to_string();
            if trimmed.is_empty() {
                return PreflightCheck {
                    name: "ort-dylib-path".to_string(),
                    passed: true,
                    detail: "ORT_DYLIB_PATH empty — runtime résoudra via artifact manifest"
                        .to_string(),
                };
            }
            let p = Path::new(&trimmed);
            if !p.exists() {
                return PreflightCheck {
                    name: "ort-dylib-path".to_string(),
                    passed: false,
                    detail: format!("ORT_DYLIB_PATH={trimmed} n existe pas sur disque"),
                };
            }
            // Heuristic V1 : la trap REQ-AXO-901630 = onnxruntime nixpkgs
            // default (pas de TensorRT EP). Le path TensorRT canonical
            // contient toujours "tensorrt" ou pointe vers .axon/ort-artifacts/.
            let lower = trimmed.to_ascii_lowercase();
            let is_nixpkgs_default = lower.contains("/nix/store/")
                && lower.contains("onnxruntime")
                && !lower.contains("tensorrt");
            if is_nixpkgs_default {
                PreflightCheck {
                    name: "ort-dylib-path".to_string(),
                    passed: false,
                    detail: format!(
                        "ORT_DYLIB_PATH={trimmed} pointe vers onnxruntime nixpkgs default \
                         (sans TensorRT EP). La trap REQ-AXO-901630. Unset et laisser le runtime \
                         résoudre via .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json"
                    ),
                }
            } else {
                PreflightCheck {
                    name: "ort-dylib-path".to_string(),
                    passed: true,
                    detail: format!("ORT_DYLIB_PATH={trimmed} (TensorRT-tagged)"),
                }
            }
        }
    }
}

fn cmd_preflight(config: InstanceConfig, json: bool) -> Result<()> {
    let pg_port: u16 = std::env::var("PGPORT")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(44144);

    let mut checks = Vec::new();
    checks.push(run_pg_isready(pg_port));

    let bin_brain = config.project_root.join("bin").join("axon-brain");
    let bin_indexer = config.project_root.join("bin").join("axon-indexer");
    checks.push(check_binary_present(&bin_brain));
    checks.push(check_binary_present(&bin_indexer));

    // REQ-AXO-901739 — env hygiene gates (Phase 2c V2).
    checks.push(check_env_no_stale_vars());
    checks.push(check_ort_dylib_path_canonical());

    let all_passed = checks.iter().all(|c| c.passed);
    let status = if all_passed { "ready" } else { "blocked" };

    let report = PreflightReport {
        instance_kind: config.instance_kind.label().to_string(),
        role: config.role.label().to_string(),
        pg_port,
        checks,
        status: status.to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "axonctl preflight — instance={} role={}",
            report.instance_kind, report.role
        );
        for c in &report.checks {
            let marker = if c.passed { "✅" } else { "❌" };
            println!("  {marker} {} — {}", c.name, c.detail);
        }
        println!("status: {}", report.status);
    }

    if !all_passed {
        return Err(anyhow!("preflight checks failed"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared process utilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ProcEntry {
    pid: i32,
    ppid: i32,
    command: String,
}

fn read_pid_file(path: &Path) -> Result<Option<i32>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(raw.trim().parse::<i32>().ok())
}

fn proc_entries() -> Result<Vec<ProcEntry>> {
    let output = Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
        .context("failed to run ps")?;
    if !output.status.success() {
        return Err(anyhow!("ps failed with status {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let mut parts = trimmed
            .splitn(3, char::is_whitespace)
            .filter(|part| !part.is_empty());
        let Some(pid_raw) = parts.next() else {
            continue;
        };
        let Some(ppid_raw) = parts.next() else {
            continue;
        };
        let command = parts.next().unwrap_or("").trim_start().to_string();
        let (Ok(pid), Ok(ppid)) = (pid_raw.parse::<i32>(), ppid_raw.parse::<i32>()) else {
            continue;
        };
        entries.push(ProcEntry { pid, ppid, command });
    }
    Ok(entries)
}

fn descendant_tree(root_pid: i32) -> Result<BTreeSet<i32>> {
    let entries = proc_entries()?;
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if !seen.insert(pid) {
            continue;
        }
        for entry in entries.iter().filter(|entry| entry.ppid == pid) {
            queue.push_back(entry.pid);
        }
    }
    Ok(seen)
}

fn process_exists(pid: i32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

fn live_pids(pids: &[i32]) -> Vec<i32> {
    pids.iter()
        .copied()
        .filter(|pid| process_exists(*pid))
        .collect()
}

fn wait_for_gone(pids: &[i32], timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if live_pids(pids).is_empty() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn terminate_pids(pids: &[i32], signal: i32) {
    for pid in pids {
        if process_exists(*pid) {
            unsafe {
                libc::kill(*pid, signal);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// REQ-AXO-116 — Rust-side socket-cleanup contract test lives in
// axonctl_tests.rs (separate file so the diff path satisfies the
// TDD guideline GUI-PRO-001 / GUI-AXO-001).
#[cfg(test)]
#[path = "axonctl_tests.rs"]
mod axonctl_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn start_argv_delegates_to_canonical_entry() {
        // REQ-AXO-901847 — the shim must delegate to scripts/axon with the
        // instance + verb + passthrough mode flags, never re-implement start.sh.
        let entry = Path::new("/home/user/projects/axon/scripts/axon");
        let argv = start_argv(entry, "dev", &["--indexer-full".to_string(), "full".to_string()]);
        assert_eq!(
            argv,
            vec![
                "bash".to_string(),
                "/home/user/projects/axon/scripts/axon".to_string(),
                "--instance".to_string(),
                "dev".to_string(),
                "start".to_string(),
                "--indexer-full".to_string(),
                "full".to_string(),
            ]
        );
    }

    #[test]
    fn start_argv_live_no_extra() {
        let entry = Path::new("/x/scripts/axon");
        let argv = start_argv(entry, "live", &[]);
        assert_eq!(argv, vec!["bash", "/x/scripts/axon", "--instance", "live", "start"]);
    }

    #[test]
    fn instance_config_dev_brain() {
        let c = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Dev,
            RuntimeRole::Brain,
        );
        assert_eq!(c.tmux_session, "axon-dev-brain");
        assert_eq!(c.elixir_node_name, "axon_dev_nexus");
        assert_eq!(c.phx_port, 44137);
        assert_eq!(c.hydra_http_port, 44139);
        assert_eq!(
            c.pid_file,
            PathBuf::from("/home/user/projects/axon/.axon-dev/run-brain/axon-brain.pid")
        );
        assert_eq!(
            c.db_root,
            PathBuf::from("/home/user/projects/axon/.axon-dev/graph_v2")
        );
        assert_eq!(c.runtime_binary_name, "axon-brain");
    }

    #[test]
    fn instance_config_live_indexer() {
        let c = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Live,
            RuntimeRole::Indexer,
        );
        assert_eq!(c.tmux_session, "axon-indexer");
        assert_eq!(c.elixir_node_name, "axon_nexus");
        assert_eq!(c.phx_port, 44127);
        assert_eq!(c.hydra_http_port, 44129);
        assert_eq!(
            c.pid_file,
            PathBuf::from("/home/user/projects/axon/.axon/run-indexer/axon-indexer.pid")
        );
        assert_eq!(c.runtime_binary_name, "axon-indexer");
    }

    #[test]
    fn instance_config_dev_indexer() {
        let c = InstanceConfig::new(
            PathBuf::from("/srv/axon"),
            InstanceKind::Dev,
            RuntimeRole::Indexer,
        );
        assert_eq!(c.tmux_session, "axon-dev-indexer");
        assert_eq!(c.elixir_node_name, "axon_dev_nexus");
        assert_eq!(c.phx_port, 44137);
        assert_eq!(
            c.pid_file,
            PathBuf::from("/srv/axon/.axon-dev/run-indexer/axon-indexer.pid")
        );
        assert_eq!(
            c.telemetry_sock,
            PathBuf::from("/tmp/axon-dev-indexer-telemetry.sock")
        );
        assert_eq!(c.mcp_sock, PathBuf::from("/tmp/axon-dev-indexer-mcp.sock"));
    }

    #[test]
    fn instance_config_live_brain() {
        let c = InstanceConfig::new(
            PathBuf::from("/srv/axon"),
            InstanceKind::Live,
            RuntimeRole::Brain,
        );
        assert_eq!(c.tmux_session, "axon-brain");
        assert_eq!(c.elixir_node_name, "axon_nexus");
        assert_eq!(c.phx_port, 44127);
        assert_eq!(
            c.pid_file,
            PathBuf::from("/srv/axon/.axon/run-brain/axon-brain.pid")
        );
    }

    #[test]
    fn process_matches_instance_runtime_binary() {
        let config = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Dev,
            RuntimeRole::Indexer,
        );
        let entry = ProcEntry {
            pid: 1,
            ppid: 0,
            command: "/home/user/projects/axon/.axon/cargo-target/debug/axon-indexer".into(),
        };
        assert!(process_matches_instance(&entry, &config));
    }

    #[test]
    fn process_matches_instance_rejects_other_project() {
        let config = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Dev,
            RuntimeRole::Indexer,
        );
        let entry = ProcEntry {
            pid: 1,
            ppid: 0,
            command: "/home/other/projects/axon/.axon/cargo-target/debug/axon-indexer".into(),
        };
        assert!(!process_matches_instance(&entry, &config));
    }

    #[test]
    fn process_matches_instance_beam_by_node_name() {
        let config = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Dev,
            RuntimeRole::Brain,
        );
        let entry = ProcEntry {
            pid: 100,
            ppid: 1,
            command: "/nix/store/xxx/beam.smp -name axon_dev_nexus@127.0.0.1 -setcookie secret"
                .into(),
        };
        assert!(process_matches_instance(&entry, &config));
    }

    #[test]
    fn process_matches_instance_rejects_other_beam_node() {
        let config = InstanceConfig::new(
            PathBuf::from("/home/user/projects/axon"),
            InstanceKind::Dev,
            RuntimeRole::Brain,
        );
        // Live instance node name — should NOT match dev config
        let entry = ProcEntry {
            pid: 100,
            ppid: 1,
            command: "/nix/store/xxx/beam.smp -name axon_nexus@127.0.0.1 -setcookie secret".into(),
        };
        assert!(!process_matches_instance(&entry, &config));
    }

    #[test]
    fn parse_lock_file_pid_extracts_owner() {
        let tmp = std::env::temp_dir().join(format!("axonctl-lock-test-{}", std::process::id()));
        fs::write(
            &tmp,
            "target=IST\nowner=axon-dev-axon-indexer;pid=12345\ndb_path=/some/path\n",
        )
        .unwrap();
        assert_eq!(parse_lock_file_pid(&tmp), Some(12345));
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn parse_lock_file_pid_returns_none_for_empty() {
        let tmp = std::env::temp_dir().join(format!("axonctl-lock-empty-{}", std::process::id()));
        fs::write(&tmp, "").unwrap();
        assert_eq!(parse_lock_file_pid(&tmp), None);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn runtime_contract_indexer_detection() {
        // REQ-AXO-902166 — gate on the indexer only when the contract names one.
        assert!(runtime_contract_has_indexer(
            r#"{"runtime_contract":"brain_mcp_indexer_ist"}"#
        ));
        assert!(!runtime_contract_has_indexer(
            r#"{"runtime_contract":"brain_only"}"#
        ));
        assert!(!runtime_contract_has_indexer("not json"));
        assert!(!runtime_contract_has_indexer(r#"{"other":"x"}"#));
    }

    #[test]
    fn indexer_health_port_defaults() {
        std::env::remove_var("AXON_INDEXER_HEALTH_PORT");
        assert_eq!(indexer_health_port(InstanceKind::Live), 44130);
        assert_eq!(indexer_health_port(InstanceKind::Dev), 44149);
    }

    // --- REQ-AXO-902165 cutover file-I/O (real manifest/bin steps, tempdir) --------

    /// Fresh, isolated (release_dir, bin_dir, archive_dir) under the temp dir.
    fn cutover_tmp(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("axonctl-cutover-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let release = root.join("live-release");
        let bin = root.join("bin");
        let archive = root.join("archive");
        for d in [&release, &bin, &archive] {
            fs::create_dir_all(d).unwrap();
        }
        (root, release, bin, archive)
    }

    #[test]
    fn cutover_stage_writes_pending_staged_and_swaps_bin() {
        let (root, release, bin, archive) = cutover_tmp("stage");
        let cand_brain = archive.join("axon-brain-new");
        fs::write(&cand_brain, b"NEW-BRAIN").unwrap();
        let candidate = serde_json::json!({
            "state": "qualified",
            "runtime_version": {"install_generation": "cand-gen", "build_id": "v-new"},
            "artifacts": {"axon-brain": {"path": cand_brain.to_string_lossy(), "sha256": "x"}}
        });
        let cand_path = root.join("candidate.json");
        write_json_file(&cand_path, &candidate).unwrap();

        cutover_stage_files(&cand_path, &release, &bin).unwrap();

        let pending = read_json_file(&release.join("pending.json")).unwrap();
        assert_eq!(pending["state"], serde_json::json!("staged"));
        assert_eq!(fs::read(bin.join("axon-brain")).unwrap(), b"NEW-BRAIN");
        // current.json is NOT touched by stage (it stays the rollback source).
        assert!(!release.join("current.json").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_finalize_archives_current_and_promotes_pending() {
        let (root, release, _bin, _archive) = cutover_tmp("finalize");
        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({"state": "promoted", "runtime_version": {"install_generation": "old-gen"}}),
        )
        .unwrap();
        write_json_file(
            &release.join("pending.json"),
            &serde_json::json!({"state": "staged", "runtime_version": {"install_generation": "new-gen"}}),
        )
        .unwrap();

        cutover_finalize_files(&release).unwrap();

        let cur = read_json_file(&release.join("current.json")).unwrap();
        assert_eq!(cur["state"], serde_json::json!("promoted"));
        assert_eq!(cur["runtime_version"]["install_generation"], serde_json::json!("new-gen"));
        assert!(release.join("history/old-gen.json").exists(), "prev generation archived");
        assert!(release.join("history/new-gen.json").exists(), "new generation archived");
        assert!(!release.join("pending.json").exists(), "pending consumed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_rollback_restores_old_bin_and_drops_pending() {
        // THE s94 recovery: after a bad staged swap, rollback must restore bin/* from
        // the untouched current.json and clear the failed pending.
        let (root, release, bin, archive) = cutover_tmp("rollback");
        let old_brain = archive.join("axon-brain-old");
        fs::write(&old_brain, b"OLD-BRAIN").unwrap();
        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({
                "state": "promoted",
                "runtime_version": {"install_generation": "old-gen"},
                "artifacts": {"axon-brain": {"path": old_brain.to_string_lossy()}}
            }),
        )
        .unwrap();
        // Simulate the failed staged swap: bin holds the NEW binary + a pending manifest.
        fs::write(bin.join("axon-brain"), b"NEW-BRAIN").unwrap();
        write_json_file(&release.join("pending.json"), &serde_json::json!({"state": "staged"})).unwrap();

        cutover_rollback_files(&release, &bin).unwrap();

        assert_eq!(fs::read(bin.join("axon-brain")).unwrap(), b"OLD-BRAIN", "bin restored to old release");
        assert!(!release.join("pending.json").exists(), "failed pending dropped");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_snapshot_errs_without_current_manifest() {
        let (root, release, _bin, _archive) = cutover_tmp("snap-none");
        assert!(cutover_snapshot(&release).is_err(), "no current.json → no rollback target");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_snapshot_errs_when_rollback_artifact_missing() {
        // Fail-fast: a current.json whose artifact source is gone cannot serve as a
        // rollback target, so snapshot must reject it before any mutation.
        let (root, release, _bin, _archive) = cutover_tmp("snap-missing");
        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({"artifacts": {"axon-brain": {"path": "/nonexistent/axon-brain"}}}),
        )
        .unwrap();
        assert!(cutover_snapshot(&release).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_snapshot_ok_when_artifacts_present() {
        let (root, release, _bin, archive) = cutover_tmp("snap-ok");
        let brain = archive.join("axon-brain");
        fs::write(&brain, b"BRAIN").unwrap();
        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({
                "runtime_version": {"install_generation": "g"},
                "artifacts": {"axon-brain": {"path": brain.to_string_lossy()}}
            }),
        )
        .unwrap();
        assert!(cutover_snapshot(&release).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cutover_stage_then_finalize_leaves_bin_and_manifest_coherent() {
        // Full happy-path file sequence: current(old) + candidate → stage → finalize.
        let (root, release, bin, archive) = cutover_tmp("stage-finalize");
        let old_brain = archive.join("axon-brain-old");
        fs::write(&old_brain, b"OLD").unwrap();
        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({
                "state": "promoted",
                "runtime_version": {"install_generation": "old-gen", "build_id": "v-old"},
                "artifacts": {"axon-brain": {"path": old_brain.to_string_lossy()}}
            }),
        )
        .unwrap();
        let new_brain = archive.join("axon-brain-new");
        fs::write(&new_brain, b"NEW").unwrap();
        let cand_path = root.join("candidate.json");
        write_json_file(
            &cand_path,
            &serde_json::json!({
                "state": "qualified",
                "runtime_version": {"install_generation": "new-gen", "build_id": "v-new"},
                "artifacts": {"axon-brain": {"path": new_brain.to_string_lossy()}}
            }),
        )
        .unwrap();

        cutover_stage_files(&cand_path, &release, &bin).unwrap();
        cutover_finalize_files(&release).unwrap();

        assert_eq!(fs::read(bin.join("axon-brain")).unwrap(), b"NEW");
        let cur = read_json_file(&release.join("current.json")).unwrap();
        assert_eq!(cur["runtime_version"]["build_id"], serde_json::json!("v-new"));
        assert_eq!(cur["state"], serde_json::json!("promoted"));
        assert!(release.join("history/old-gen.json").exists());
        assert!(!release.join("pending.json").exists());
        let _ = fs::remove_dir_all(&root);
    }

    // REQ-AXO-902165 E2E (fault injection) — the auto-rollback proven against REAL
    // processes + a REAL HTTP /readyz probe, not just the FakeIo decision flow. A
    // BROKEN candidate (a binary that never serves) must fail the health-gate; the
    // auto-rollback must restore the GOOD binary, which then serves again. Exercises
    // drive_cutover + the real file steps + real spawn/kill + real probe end-to-end;
    // only RealCutoverIo's scripts/axon-devenv wiring is out of scope (that seam needs
    // a full dev/live runtime). #[ignore] — spawns processes + binds a port; run
    // explicitly: `cargo test --bin axonctl -- --ignored cutover_e2e`.
    struct TestProcCutoverIo {
        release_dir: PathBuf,
        bin_dir: PathBuf,
        candidate: PathBuf,
        port: u16,
        child: Option<std::process::Child>,
    }

    impl TestProcCutoverIo {
        fn kill_child(&mut self) {
            if let Some(mut c) = self.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        /// Launch whatever binary currently sits at bin/axon-brain (good OR broken).
        fn spawn_swapped(&mut self) {
            let bin = self.bin_dir.join("axon-brain");
            self.child = Command::new("bash")
                .arg(&bin)
                .arg(self.port.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok();
        }
    }

    impl CutoverIo for TestProcCutoverIo {
        fn snapshot_current(&mut self) -> Result<(), String> {
            cutover_snapshot(&self.release_dir).map(|_| ()).map_err(|e| e.to_string())
        }
        fn stage_candidate(&mut self) -> Result<(), String> {
            cutover_stage_files(&self.candidate, &self.release_dir, &self.bin_dir)
                .map_err(|e| e.to_string())
        }
        fn restart_runtime(&mut self) -> Result<(), String> {
            self.kill_child();
            self.spawn_swapped();
            Ok(())
        }
        fn finalize(&mut self) -> Result<(), String> {
            cutover_finalize_files(&self.release_dir).map_err(|e| e.to_string())
        }
        fn rollback(&mut self) -> Result<(), String> {
            cutover_rollback_files(&self.release_dir, &self.bin_dir).map_err(|e| e.to_string())?;
            self.kill_child();
            self.spawn_swapped(); // bin/axon-brain is the GOOD binary again
            Ok(())
        }
    }

    #[test]
    #[ignore]
    fn cutover_e2e_broken_candidate_auto_rolls_back_to_serving_old() {
        use std::os::unix::fs::PermissionsExt;
        let (root, release, bin, archive) = cutover_tmp("e2e");
        let port = 47231u16;

        // GOOD binary: a bash script that serves HTTP 200 on any path (stand-in /readyz).
        // BROKEN binary: exits immediately → the health-gate can never pass.
        let good = archive.join("axon-brain-good");
        let good_script = "#!/usr/bin/env bash\nexec python3 -c \"import http.server as h; port=int('__PORT__'); s=h.HTTPServer(('127.0.0.1',port), type('H',(h.BaseHTTPRequestHandler,),{'do_GET':lambda self:(self.send_response(200),self.end_headers()),'log_message':lambda *a:None})); s.serve_forever()\"\n".replace("__PORT__", "$1");
        fs::write(&good, good_script).unwrap();
        let broken = archive.join("axon-brain-broken");
        fs::write(&broken, "#!/usr/bin/env bash\nexit 1\n").unwrap();
        for p in [&good, &broken] {
            let mut perm = fs::metadata(p).unwrap().permissions();
            perm.set_mode(0o755);
            fs::set_permissions(p, perm).unwrap();
        }

        write_json_file(
            &release.join("current.json"),
            &serde_json::json!({
                "state": "promoted",
                "runtime_version": {"install_generation": "old-gen"},
                "artifacts": {"axon-brain": {"path": good.to_string_lossy()}}
            }),
        )
        .unwrap();
        let candidate = root.join("candidate.json");
        write_json_file(
            &candidate,
            &serde_json::json!({
                "state": "qualified",
                "runtime_version": {"install_generation": "new-gen"},
                "artifacts": {"axon-brain": {"path": broken.to_string_lossy()}}
            }),
        )
        .unwrap();

        let mut io = TestProcCutoverIo {
            release_dir: release.clone(),
            bin_dir: bin.clone(),
            candidate,
            port,
            child: None,
        };
        let url = format!("http://127.0.0.1:{port}/readyz");
        let probe = || http_ready(&url, 1);
        let wait = || thread::sleep(Duration::from_millis(300));

        let verdict = drive_cutover(&mut io, probe, 3, wait);

        // The broken candidate never served → auto-rollback (never finalize).
        assert!(
            matches!(
                verdict,
                CutoverVerdict::RolledBack { failed_step: "health_gate", rollback_ok: true, .. }
            ),
            "expected auto-rollback on a broken candidate, got {verdict:?}"
        );
        // bin/axon-brain restored to the GOOD binary.
        assert_eq!(
            fs::read(bin.join("axon-brain")).unwrap(),
            fs::read(&good).unwrap(),
            "rollback must restore the GOOD binary in bin/"
        );
        // The GOOD runtime serves /readyz again after rollback (poll up to ~3s).
        let mut served = false;
        for _ in 0..30 {
            if http_ready(&url, 1) {
                served = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        io.kill_child();
        let _ = fs::remove_dir_all(&root);
        assert!(served, "old GOOD runtime must serve /readyz again after auto-rollback");
    }

    #[test]
    fn descendant_tree_finds_self() {
        let my_pid = std::process::id() as i32;
        let tree = descendant_tree(my_pid).unwrap();
        assert!(
            tree.contains(&my_pid),
            "tree should contain self: {:?}",
            tree
        );
    }

    /// Integration test: verifies parent-first SIGTERM ordering prevents child respawn.
    #[test]
    #[ignore]
    fn stop_tree_parent_first_prevents_respawn() {
        let tmp = std::env::temp_dir().join(format!("axonctl-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let pid_file = tmp.join("test.pid");
        let parent_ts = tmp.join("parent_sigterm_ts");
        let child_ts = tmp.join("child_sigterm_ts");
        let respawn_marker = tmp.join("respawn_attempted");

        let parent_script = format!(
            r#"#!/bin/bash
trap 'date +%s%N > {parent_ts}; exit 0' TERM
bash -c 'trap "date +%s%N > {child_ts}; exit 0" TERM; sleep 300' &
CHILD_PID=$!
echo $$ > {pid_file}
wait $CHILD_PID 2>/dev/null
touch {respawn_marker}
bash -c 'sleep 300' &
wait
"#,
            parent_ts = parent_ts.display(),
            child_ts = child_ts.display(),
            pid_file = pid_file.display(),
            respawn_marker = respawn_marker.display(),
        );

        let script_file = tmp.join("parent.sh");
        {
            let mut f = std::fs::File::create(&script_file).unwrap();
            f.write_all(parent_script.as_bytes()).unwrap();
        }

        let mut parent = Command::new("bash")
            .arg(&script_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn parent");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(pid_file.exists(), "parent did not write PID file");

        let root_pid = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();

        thread::sleep(Duration::from_millis(200));

        let tree = descendant_tree(root_pid).expect("failed to discover descendants");
        assert!(tree.len() >= 2, "expected parent + child, got {:?}", tree);

        let root_pids = vec![root_pid];
        let child_pids: Vec<i32> = tree
            .iter()
            .copied()
            .filter(|pid| *pid != root_pid)
            .rev()
            .collect();

        terminate_pids(&root_pids, libc::SIGTERM);
        thread::sleep(Duration::from_millis(200));
        terminate_pids(&child_pids, libc::SIGTERM);

        let all_pids: Vec<i32> = tree.iter().copied().collect();
        wait_for_gone(&all_pids, Duration::from_secs(3));

        let remaining = live_pids(&all_pids);
        if !remaining.is_empty() {
            terminate_pids(&remaining, libc::SIGKILL);
            wait_for_gone(&remaining, Duration::from_millis(500));
        }

        let _ = parent.wait();

        let final_remaining = live_pids(&all_pids);
        assert!(
            final_remaining.is_empty(),
            "processes still alive: {:?}",
            final_remaining
        );

        assert!(parent_ts.exists(), "parent did not record SIGTERM receipt");

        if child_ts.exists() {
            let pts: u64 = std::fs::read_to_string(&parent_ts)
                .unwrap()
                .trim()
                .parse()
                .unwrap_or(0);
            let cts: u64 = std::fs::read_to_string(&child_ts)
                .unwrap()
                .trim()
                .parse()
                .unwrap_or(0);
            assert!(
                pts <= cts,
                "parent SIGTERM ({}) should arrive before or at same time as child ({})",
                pts,
                cts
            );
        }

        assert!(
            !respawn_marker.exists(),
            "parent attempted to respawn child after SIGTERM — parent-first ordering failed"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
