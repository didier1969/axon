use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
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
            _ => Err(anyhow!("invalid --instance-kind: `{s}` (expected dev|live)")),
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
            _ => Err(anyhow!("invalid --role: `{s}` (expected brain|indexer|all)")),
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
        let run_root = project_root.join(instance_dir).join(format!("run-{role_label}"));
        let db_root = project_root.join(instance_dir).join("graph_v2");
        let tmux_session = match instance_kind {
            InstanceKind::Dev => format!("axon-dev-{role_label}"),
            InstanceKind::Live => format!("axon-{role_label}"),
        };
        let telemetry_sock =
            PathBuf::from(format!("/tmp/axon-{instance_label}-{role_label}-telemetry.sock"));
        let mcp_sock =
            PathBuf::from(format!("/tmp/axon-{instance_label}-{role_label}-mcp.sock"));

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
  stop          Orchestrated instance stop (kill processes, clean locks, verify)
  supervise     Spawn and supervise a runtime binary (signal forwarding, PID file)
  status        Health check for an instance
  auto-restart  REQ-AXO-097: poll role health, respawn on failure detection

Options:
  --project-root PATH     Axon project root directory
  --instance-kind KIND    dev or live
  --role ROLE             brain, indexer, or all
  --json                  Machine-readable JSON output
  --hard                  (stop only) Aggressive cleanup with port-based kill
  --timeout-ms N          (stop) SIGTERM grace period in ms (default 15000)
  --interval-ms N         (auto-restart) Poll cadence in ms (default 5000)
  --max-restarts N        (auto-restart) Cap on restart attempts (default unbounded)
  --grace-ms N            (auto-restart) Grace period after restart before next probe (default 30000)
  -- EXECUTABLE [ARGS]    (supervise/auto-restart) Binary to spawn (and re-spawn)
"
}

struct GlobalArgs {
    project_root: Option<PathBuf>,
    instance_kind: Option<String>,
    role: Option<String>,
    json: bool,
    hard: bool,
    timeout_ms: u64,
    /// REQ-AXO-097 — auto-restart polling cadence in ms.
    interval_ms: u64,
    /// REQ-AXO-097 — auto-restart attempt cap. None = unbounded.
    max_restarts: Option<u32>,
    /// REQ-AXO-097 — grace window after spawning the restart command
    /// before resuming polling, so a slow start does not trigger a
    /// second restart attempt.
    grace_ms: u64,
    remaining: Vec<String>,
    passthrough: Vec<String>,
}

fn parse_global_args(raw: Vec<String>) -> Result<(String, GlobalArgs)> {
    let mut iter = raw.into_iter();
    let command = iter
        .next()
        .ok_or_else(|| anyhow!("{}", usage()))?;

    let mut args = GlobalArgs {
        project_root: None,
        instance_kind: None,
        role: None,
        json: false,
        hard: false,
        timeout_ms: 15_000,
        interval_ms: 5_000,
        max_restarts: None,
        grace_ms: 30_000,
        remaining: Vec::new(),
        passthrough: Vec::new(),
    };

    let mut saw_separator = false;
    while let Some(arg) = iter.next() {
        if saw_separator {
            args.passthrough.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => saw_separator = true,
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
            "--interval-ms" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--interval-ms requires a value"))?;
                args.interval_ms = value
                    .parse::<u64>()
                    .context("--interval-ms must be a positive integer")?;
            }
            "--max-restarts" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--max-restarts requires a value"))?;
                args.max_restarts = Some(
                    value
                        .parse::<u32>()
                        .context("--max-restarts must be a non-negative integer")?,
                );
            }
            "--grace-ms" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--grace-ms requires a value"))?;
                args.grace_ms = value
                    .parse::<u64>()
                    .context("--grace-ms must be a positive integer")?;
            }
            "--help" | "-h" => return Err(anyhow!("{}", usage())),
            _ => args.remaining.push(arg),
        }
    }
    // Collect passthrough remainder
    args.passthrough.extend(iter);

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
        "supervise" => cmd_supervise(require_config(&args)?, args.passthrough),
        "auto-restart" => cmd_auto_restart(
            require_config(&args)?,
            args.passthrough,
            args.interval_ms,
            args.max_restarts,
            args.grace_ms,
            args.json,
        ),
        "status" => {
            let base = require_config(&args)?;
            let roles = base.role.concrete_roles();
            for role in roles {
                let cfg = InstanceConfig::new(base.project_root.clone(), base.instance_kind, role);
                cmd_status(cfg, args.json)?;
            }
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(anyhow!("unknown command `{other}`\n{}", usage())),
    }
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
                    detail: format!(
                        "BEAM processes matching node {}",
                        config.elixir_node_name
                    ),
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
    cleanup_files(&[
        &config.telemetry_sock,
        &config.mcp_sock,
        &config.pid_file,
    ]);

    // Step 8: Final verification
    let final_remaining = find_instance_all_pids(&config);

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
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if final_remaining.is_empty() {
        println!(
            "axonctl stop: {} {} stopped cleanly",
            report.instance_kind, report.role
        );
    } else {
        eprintln!(
            "axonctl stop: {} {} has remaining pids: {:?}",
            report.instance_kind, report.role, final_remaining
        );
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
    let output = Command::new("ss")
        .args(["-ltnp"])
        .output()
        .ok();
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

// ---------------------------------------------------------------------------
// Phase 2: axonctl supervise — spawn + signal forward + waitpid
// ---------------------------------------------------------------------------

fn cmd_supervise(config: InstanceConfig, passthrough: Vec<String>) -> Result<()> {
    if passthrough.is_empty() {
        return Err(anyhow!(
            "supervise requires an executable after --\n\
             Usage: axonctl supervise ... -- EXECUTABLE [ARGS]"
        ));
    }

    let executable = &passthrough[0];
    let extra_args = &passthrough[1..];

    // Ensure run root exists
    fs::create_dir_all(&config.run_root)
        .with_context(|| format!("failed to create run root {}", config.run_root.display()))?;

    // Spawn child process
    let child = std::process::Command::new(executable)
        .args(extra_args)
        .spawn()
        .with_context(|| format!("failed to spawn {executable}"))?;

    let child_pid = child.id() as i32;

    // Write PID file atomically (write tmp then rename)
    let pid_tmp = config.pid_file.with_extension("pid.tmp");
    fs::write(&pid_tmp, format!("{child_pid}\n"))
        .with_context(|| format!("failed to write PID file {}", pid_tmp.display()))?;
    fs::rename(&pid_tmp, &config.pid_file)
        .with_context(|| format!("failed to rename PID file to {}", config.pid_file.display()))?;

    eprintln!(
        "axonctl supervise: spawned {} (pid={child_pid}), pid file: {}",
        executable,
        config.pid_file.display()
    );

    // Install signal handlers to forward SIGTERM/SIGINT to child
    install_signal_forwarding(child_pid);

    // Wait for child using waitpid (proper reaping, no polling)
    let exit_code = wait_for_child(child_pid);

    // Cleanup PID file
    let _ = fs::remove_file(&config.pid_file);

    eprintln!("axonctl supervise: child exited with code {exit_code}");
    std::process::exit(exit_code);
}

fn install_signal_forwarding(child_pid: i32) {
    // Store child PID in a global atomic for the signal handler
    SUPERVISED_CHILD_PID.store(child_pid, std::sync::atomic::Ordering::SeqCst);

    unsafe {
        libc::signal(libc::SIGTERM, signal_forward_handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, signal_forward_handler as *const () as libc::sighandler_t);
    }
}

static SUPERVISED_CHILD_PID: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(-1);

extern "C" fn signal_forward_handler(sig: libc::c_int) {
    let pid = SUPERVISED_CHILD_PID.load(std::sync::atomic::Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

fn wait_for_child(child_pid: i32) -> i32 {
    let mut status: libc::c_int = 0;
    loop {
        let result = unsafe { libc::waitpid(child_pid, &mut status, 0) };
        if result == child_pid {
            if libc::WIFEXITED(status) {
                return libc::WEXITSTATUS(status);
            }
            if libc::WIFSIGNALED(status) {
                return 128 + libc::WTERMSIG(status);
            }
            return 1;
        }
        if result == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                // EINTR from signal handler — retry waitpid
                continue;
            }
            // ECHILD or other error — child already reaped
            return 1;
        }
    }
}

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
    /// Examples: `mcp_socket_missing` for a brain instance whose primary
    /// public surface is the MCP HTTP/socket endpoint;
    /// `telemetry_socket_missing` for an indexer whose telemetry stream is
    /// part of its operator contract. When this list is non-empty, `overall`
    /// is downgraded to `degraded` even if the process is alive.
    role_contract_violations: Vec<String>,
    overall: String,
}

/// REQ-AXO-151 — given the runtime role and the observed socket state, return
/// the role contract items the instance is failing to satisfy. Returns an
/// empty vec when the contract is whole. Pure function for unit testing.
fn compute_role_contract_violations(role: RuntimeRole, sockets: &[SocketStatus]) -> Vec<String> {
    let socket_present = |name: &str| -> bool {
        sockets
            .iter()
            .any(|s| s.name == name && s.exists)
    };
    let mut violations = Vec::new();
    match role {
        RuntimeRole::Brain => {
            if !socket_present("mcp") {
                violations.push("mcp_socket_missing".to_string());
            }
        }
        RuntimeRole::Indexer => {
            if !socket_present("telemetry") {
                violations.push("telemetry_socket_missing".to_string());
            }
        }
        RuntimeRole::All => {
            if !socket_present("mcp") {
                violations.push("mcp_socket_missing".to_string());
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

    // REQ-AXO-151 — role contract: brain MUST expose its MCP socket;
    // indexer MUST expose its telemetry socket. A live process whose role
    // contract is broken is `degraded`, never `healthy`. Without this gate,
    // a brain that lost its MCP socket (e.g. the 2026-05-03 promotion that
    // restarted live in indexer-graph mode) reports `healthy` while serving
    // no MCP — misleading both human operators and LLM clients parsing JSON.
    let role_contract_violations = compute_role_contract_violations(config.role, &sockets);

    let overall = if !alive {
        "down"
    } else if !cmdline_matches {
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
            println!("  process: pid={pid} alive={} match={}", report.process.alive, report.process.cmdline_matches);
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
                println!(
                    "  guard {}: {} (pid={:?})",
                    g.target, state, g.owner_pid
                );
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
// Health probe is the same liveness logic used by `cmd_status`: pid
// file present, kill -0 succeeds, cmdline matches the instance.
// Anything weaker (e.g. pid file alone) would re-spawn against a
// reused PID and double-up.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoleHealth {
    /// Process alive AND cmdline matches the instance signature.
    Healthy,
    /// PID file points at a live process whose cmdline does NOT
    /// match the instance signature (PID reuse): the role is gone,
    /// some other process now owns that PID.
    PidReused,
    /// PID file present but the process is gone (stale pidfile).
    Dead,
    /// No PID file on disk: never started, or stop ran cleanly.
    Absent,
}

impl RoleHealth {
    /// Whether the role is OK and no action is required. Any other
    /// state (Dead, PidReused, Absent) is treated by `auto-restart`
    /// as "needs restart": the operator's intent in invoking the
    /// command is "keep this role running" regardless of which
    /// way it went down.
    pub(crate) fn is_healthy(self) -> bool {
        matches!(self, RoleHealth::Healthy)
    }
}

pub(crate) fn role_health(config: &InstanceConfig) -> RoleHealth {
    let pid_file_exists = config.pid_file.exists();
    let pid = if pid_file_exists {
        read_pid_file(&config.pid_file).ok().flatten()
    } else {
        None
    };
    let alive = pid.map(process_exists).unwrap_or(false);
    if !pid_file_exists {
        return RoleHealth::Absent;
    }
    if !alive {
        return RoleHealth::Dead;
    }
    let pid = pid.expect("alive implies pid present");
    if process_cmdline_matches_instance(pid, config) {
        RoleHealth::Healthy
    } else {
        RoleHealth::PidReused
    }
}

#[derive(Debug, Serialize)]
struct AutoRestartTickEvent<'a> {
    event: &'a str,
    instance_kind: &'a str,
    role: &'a str,
    health: RoleHealth,
    restart_count: u32,
    max_restarts: Option<u32>,
}

fn emit_tick_event<'a>(json: bool, event: &'a str, config: &InstanceConfig, health: RoleHealth, restart_count: u32, max_restarts: Option<u32>) {
    if json {
        let payload = AutoRestartTickEvent {
            event,
            instance_kind: config.instance_kind.label(),
            role: config.role.label(),
            health,
            restart_count,
            max_restarts,
        };
        if let Ok(line) = serde_json::to_string(&payload) {
            eprintln!("{line}");
        }
    } else {
        eprintln!(
            "axonctl auto-restart: {event} {} {} health={:?} restarts={}/{}",
            config.instance_kind.label(),
            config.role.label(),
            health,
            restart_count,
            match max_restarts {
                Some(n) => n.to_string(),
                None => "∞".into(),
            }
        );
    }
}

fn cmd_auto_restart(
    config: InstanceConfig,
    restart_command: Vec<String>,
    interval_ms: u64,
    max_restarts: Option<u32>,
    grace_ms: u64,
    json: bool,
) -> Result<()> {
    if restart_command.is_empty() {
        return Err(anyhow!(
            "auto-restart requires a restart command after --\n\
             Usage: axonctl auto-restart ... -- EXECUTABLE [ARGS]"
        ));
    }
    if interval_ms < 100 {
        return Err(anyhow!(
            "--interval-ms must be ≥ 100; got {interval_ms} (faster polling burns CPU without observing real signal)"
        ));
    }

    let interval = Duration::from_millis(interval_ms);
    let grace = Duration::from_millis(grace_ms);
    let mut restart_count: u32 = 0;
    emit_tick_event(json, "auto_restart_started", &config, role_health(&config), restart_count, max_restarts);

    loop {
        let health = role_health(&config);
        if health.is_healthy() {
            thread::sleep(interval);
            continue;
        }
        if matches!(health, RoleHealth::Absent) {
            // Never started, or stopped cleanly. Still try to start
            // (operator semantics: auto-restart implies "keep this
            // role running"), but cap by max_restarts.
        }
        if let Some(max) = max_restarts {
            if restart_count >= max {
                emit_tick_event(json, "auto_restart_cap_reached", &config, health, restart_count, max_restarts);
                return Err(anyhow!(
                    "auto-restart cap reached ({max} attempts); giving up"
                ));
            }
        }
        restart_count = restart_count.saturating_add(1);
        emit_tick_event(json, "auto_restart_spawn", &config, health, restart_count, max_restarts);

        // Spawn restart command without waiting — it is expected to
        // be a `start` script that detaches its own runtime. We do
        // wait for the immediate fork to return so we observe spawn
        // failures (bad path, etc.).
        let executable = &restart_command[0];
        let extra_args = &restart_command[1..];
        let spawn_result = Command::new(executable).args(extra_args).spawn();
        match spawn_result {
            Ok(mut child) => {
                // Poll the spawned process briefly; for a script that
                // forks-and-exits, the wait returns quickly. For a
                // long-running supervisor, we stop waiting after the
                // grace window and re-enter the polling loop.
                let waited_at = Instant::now();
                while waited_at.elapsed() < grace {
                    match child.try_wait() {
                        Ok(Some(_status)) => break,
                        Ok(None) => thread::sleep(Duration::from_millis(200)),
                        Err(_) => break,
                    }
                }
            }
            Err(err) => {
                emit_tick_event(json, "auto_restart_spawn_failed", &config, health, restart_count, max_restarts);
                return Err(anyhow!(
                    "auto-restart failed to spawn `{executable}`: {err}"
                ));
            }
        }

        // Grace window: wait for the runtime to come back up
        // before we observe it as "still dead" and double-restart.
        thread::sleep(grace);
    }
}

fn get_listening_ports() -> BTreeSet<u16> {
    let output = Command::new("ss")
        .args(["-ltn"])
        .output()
        .ok();
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
        assert_eq!(
            c.mcp_sock,
            PathBuf::from("/tmp/axon-dev-indexer-mcp.sock")
        );
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
            command: "/nix/store/xxx/beam.smp -name axon_dev_nexus@127.0.0.1 -setcookie secret".into(),
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

        assert!(
            parent_ts.exists(),
            "parent did not record SIGTERM receipt"
        );

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
