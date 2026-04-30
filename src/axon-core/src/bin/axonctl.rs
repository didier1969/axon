use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct StopTreeArgs {
    pid_file: PathBuf,
    project_root: PathBuf,
    runtime_name: String,
    launcher_name: String,
    timeout_ms: u64,
    json: bool,
}

#[derive(Debug, Serialize)]
struct StopTreeReport {
    root_pid: Option<i32>,
    killed_pids: Vec<i32>,
    remaining_pids: Vec<i32>,
    status: String,
}

#[derive(Debug, Clone)]
struct ProcEntry {
    pid: i32,
    ppid: i32,
    command: String,
}

fn usage() -> &'static str {
    "Usage:\n  axonctl stop-tree --pid-file PATH --project-root PATH --runtime-name NAME [--launcher-name NAME] [--timeout-ms N] [--json]\n"
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(anyhow!("{}", usage()));
    };

    match command.as_str() {
        "stop-tree" => stop_tree(parse_stop_tree(args.collect())?),
        "--help" | "-h" | "help" => {
            print!("{}", usage());
            Ok(())
        }
        other => Err(anyhow!("unknown axonctl command `{other}`\n{}", usage())),
    }
}

fn parse_stop_tree(raw: Vec<String>) -> Result<StopTreeArgs> {
    let mut pid_file = None;
    let mut project_root = None;
    let mut runtime_name = None;
    let mut launcher_name = None;
    let mut timeout_ms = 1_500_u64;
    let mut json = false;
    let mut iter = raw.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--pid-file" => pid_file = iter.next().map(PathBuf::from),
            "--project-root" => project_root = iter.next().map(PathBuf::from),
            "--runtime-name" => runtime_name = iter.next(),
            "--launcher-name" => launcher_name = iter.next(),
            "--timeout-ms" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--timeout-ms requires a value"))?;
                timeout_ms = value
                    .parse::<u64>()
                    .context("--timeout-ms must be a positive integer")?;
            }
            "--json" => json = true,
            "--help" | "-h" => return Err(anyhow!("{}", usage())),
            other => return Err(anyhow!("unknown stop-tree option `{other}`\n{}", usage())),
        }
    }

    let runtime_name = runtime_name.ok_or_else(|| anyhow!("--runtime-name is required"))?;
    Ok(StopTreeArgs {
        pid_file: pid_file.ok_or_else(|| anyhow!("--pid-file is required"))?,
        project_root: project_root.ok_or_else(|| anyhow!("--project-root is required"))?,
        launcher_name: launcher_name.unwrap_or_else(|| format!("launch-{runtime_name}.sh")),
        runtime_name,
        timeout_ms,
        json,
    })
}

fn stop_tree(args: StopTreeArgs) -> Result<()> {
    let root_pid = read_pid_file(&args.pid_file)?;
    let mut candidate_pids = BTreeSet::new();

    if let Some(pid) = root_pid {
        if process_exists(pid) {
            candidate_pids.extend(descendant_tree(pid)?);
        }
    }

    for entry in proc_entries()? {
        if process_matches_runtime(
            &entry,
            &args.project_root,
            &args.runtime_name,
            &args.launcher_name,
        ) {
            candidate_pids.insert(entry.pid);
            candidate_pids.extend(descendant_tree(entry.pid)?);
        }
    }

    // Collect all PIDs for the final report (ascending order for display).
    let all_pids: Vec<i32> = candidate_pids.iter().copied().collect();

    // Phase 1: SIGTERM root parents first so they stop respawning GPU children.
    // Root PIDs are the ones tracked from the pid file or matched directly by
    // process_matches_runtime (lowest PIDs, typically the parent processes).
    let mut root_pids = Vec::new();
    if let Some(pid) = root_pid {
        if candidate_pids.contains(&pid) {
            root_pids.push(pid);
        }
    }
    for entry in proc_entries().unwrap_or_default() {
        if process_matches_runtime(
            &entry,
            &args.project_root,
            &args.runtime_name,
            &args.launcher_name,
        ) && candidate_pids.contains(&entry.pid)
        {
            root_pids.push(entry.pid);
        }
    }
    root_pids.sort_unstable();
    root_pids.dedup();

    let child_pids: Vec<i32> = all_pids
        .iter()
        .copied()
        .filter(|pid| !root_pids.contains(pid))
        .rev()
        .collect();

    // Send SIGTERM to roots first, brief grace for shutdown initiation,
    // then SIGTERM remaining children to accelerate teardown.
    terminate_pids(&root_pids, libc::SIGTERM);
    if !child_pids.is_empty() {
        thread::sleep(Duration::from_millis(200));
        terminate_pids(&child_pids, libc::SIGTERM);
    }

    wait_for_gone(&all_pids, Duration::from_millis(args.timeout_ms));
    let remaining_after_term = live_pids(&all_pids);
    terminate_pids(&remaining_after_term, libc::SIGKILL);
    wait_for_gone(&remaining_after_term, Duration::from_millis(500));
    let remaining_pids = live_pids(&all_pids);

    let report = StopTreeReport {
        root_pid,
        killed_pids: all_pids,
        status: if remaining_pids.is_empty() {
            "stopped".to_string()
        } else {
            "remaining".to_string()
        },
        remaining_pids,
    };

    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else if report.remaining_pids.is_empty() {
        println!("axonctl stop-tree: stopped pids {:?}", report.killed_pids);
    } else {
        println!(
            "axonctl stop-tree: remaining pids {:?} after killing {:?}",
            report.remaining_pids, report.killed_pids
        );
    }

    if report.remaining_pids.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "processes still alive: {:?}",
            report.remaining_pids
        ))
    }
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

fn process_matches_runtime(
    entry: &ProcEntry,
    project_root: &Path,
    runtime_name: &str,
    launcher_name: &str,
) -> bool {
    let root = project_root.to_string_lossy();
    if !entry.command.contains(root.as_ref()) {
        return false;
    }
    entry.command.contains(runtime_name) || entry.command.contains(launcher_name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Integration test: verifies parent-first SIGTERM ordering prevents child respawn.
    ///
    /// Spawns a parent shell that spawns a child, both recording SIGTERM receipt
    /// timestamps to temp files. Runs axonctl logic to kill them parent-first.
    /// Asserts: parent got SIGTERM first, child got it after, no respawn occurred.
    #[test]
    #[ignore] // Requires real processes and /proc — run with: cargo test --bins -- --ignored
    fn stop_tree_parent_first_prevents_respawn() {
        let tmp = std::env::temp_dir().join(format!("axonctl-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let pid_file = tmp.join("test.pid");
        let parent_ts = tmp.join("parent_sigterm_ts");
        let child_ts = tmp.join("child_sigterm_ts");
        let respawn_marker = tmp.join("respawn_attempted");

        // Parent script: spawns a child, installs SIGTERM handler that records timestamp,
        // and attempts to respawn child after SIGTERM (which should fail because parent exits).
        let parent_script = format!(
            r#"#!/bin/bash
trap 'date +%s%N > {parent_ts}; exit 0' TERM
# Spawn child
bash -c 'trap "date +%s%N > {child_ts}; exit 0" TERM; sleep 300' &
CHILD_PID=$!
echo $$ > {pid_file}
# Wait for SIGTERM
wait $CHILD_PID 2>/dev/null
# If we get here after SIGTERM, try to respawn (should not succeed if parent is exiting)
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

        // Start parent process
        let mut parent = Command::new("bash")
            .arg(&script_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn parent");

        // Wait for PID file to appear (parent ready)
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

        // Small delay so child is fully up
        thread::sleep(Duration::from_millis(200));

        // Discover process tree
        let tree = descendant_tree(root_pid).expect("failed to discover descendants");
        assert!(tree.len() >= 2, "expected parent + child, got {:?}", tree);

        // Identify root vs children
        let root_pids = vec![root_pid];
        let child_pids: Vec<i32> = tree
            .iter()
            .copied()
            .filter(|pid| *pid != root_pid)
            .rev()
            .collect();

        // Phase 1: SIGTERM parent first
        terminate_pids(&root_pids, libc::SIGTERM);

        // Phase 2: Grace period then SIGTERM children
        thread::sleep(Duration::from_millis(200));
        terminate_pids(&child_pids, libc::SIGTERM);

        // Wait for all to exit
        let all_pids: Vec<i32> = tree.iter().copied().collect();
        wait_for_gone(&all_pids, Duration::from_secs(3));

        // Escalate if needed
        let remaining = live_pids(&all_pids);
        if !remaining.is_empty() {
            terminate_pids(&remaining, libc::SIGKILL);
            wait_for_gone(&remaining, Duration::from_millis(500));
        }

        // Also reap spawned process
        let _ = parent.wait();

        // Assertions
        let final_remaining = live_pids(&all_pids);
        assert!(
            final_remaining.is_empty(),
            "processes still alive: {:?}",
            final_remaining
        );

        // Verify parent received SIGTERM (timestamp file exists)
        assert!(
            parent_ts.exists(),
            "parent did not record SIGTERM receipt"
        );

        // If child timestamp exists, verify parent got SIGTERM first
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

        // Verify no respawn was attempted (parent should have exited before reaching respawn)
        assert!(
            !respawn_marker.exists(),
            "parent attempted to respawn child after SIGTERM — parent-first ordering failed"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
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

    #[test]
    fn process_matches_runtime_checks_both_fields() {
        let entry = ProcEntry {
            pid: 1,
            ppid: 0,
            command: "/home/user/projects/axon/target/debug/axon-indexer --foo".to_string(),
        };
        assert!(process_matches_runtime(
            &entry,
            Path::new("/home/user/projects/axon"),
            "axon-indexer",
            "axon-launcher",
        ));
        assert!(!process_matches_runtime(
            &entry,
            Path::new("/home/other/project"),
            "axon-indexer",
            "axon-launcher",
        ));
    }
}
