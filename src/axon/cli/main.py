"""Axon CLI — Graph-powered code intelligence engine."""

from __future__ import annotations

import fcntl
import hashlib
import json
import logging
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.progress import Progress, SpinnerColumn, TextColumn

from axon import __version__

console = Console()
logger = logging.getLogger(__name__)

from axon.core.paths import central_db_path as _central_db_path, compute_repo_slug  # noqa: E402


def _auto_migrate_local_kuzu(repo_path: Path, slug: str) -> None:
    """Move {repo_path}/.axon/kuzu → ~/.axon/repos/{slug}/kuzu if needed.

    Skips if central DB already exists or local DB doesn't exist.
    Logs at INFO when migration happens.
    """
    local_kuzu = repo_path / ".axon" / "kuzu"
    central = _central_db_path(slug)
    if not local_kuzu.exists() or central.exists():
        return
    central.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(local_kuzu), str(central))
    logger.info("Migrated KuzuDB: %s → %s", local_kuzu, central)


def _load_storage(repo_path: Path | None = None) -> "KuzuBackend":  # noqa: F821
    """Load the KuzuDB backend for the given or current repo."""
    from axon.core.storage.kuzu_backend import KuzuBackend

    target = (repo_path or Path.cwd()).resolve()
    axon_dir = target / ".axon"
    meta_path = axon_dir / "meta.json"

    # Determine DB path: central (slug-based) or legacy fallback
    db_path: Path | None = None
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text(encoding="utf-8"))
            slug = meta.get("slug")
            if slug:
                db_path = _central_db_path(slug)
        except (json.JSONDecodeError, OSError):
            pass
    # Legacy fallback (repos indexed before v0.6)
    if db_path is None:
        db_path = axon_dir / "kuzu"

    if not db_path.exists():
        console.print(
            f"[red]Error:[/red] No index found at {target}. Run 'axon analyze' first."
        )
        raise typer.Exit(code=1)

    storage = KuzuBackend()
    storage.initialize(db_path, read_only=True)
    return storage


def _register_in_global_registry(meta: dict, repo_path: Path) -> None:
    """Write meta.json into ``~/.axon/repos/{slug}/`` for multi-repo discovery.

    Slug is ``{repo_name}`` if that slot is unclaimed or already belongs to
    this repo.  Falls back to ``{repo_name}-{sha256(path)[:8]}`` on collision.
    """
    registry_root = Path.home() / ".axon" / "repos"
    repo_name = repo_path.name

    candidate = registry_root / repo_name
    slug = repo_name
    if candidate.exists():
        existing_meta_path = candidate / "meta.json"
        try:
            existing = json.loads(existing_meta_path.read_text())
            if existing.get("path") != str(repo_path):
                short_hash = hashlib.sha256(str(repo_path).encode()).hexdigest()[:8]
                slug = f"{repo_name}-{short_hash}"
        except (json.JSONDecodeError, OSError):
            shutil.rmtree(candidate, ignore_errors=True)  # Clean broken slot before claiming

    # Remove any stale entry for the same repo_path under a different slug.
    if registry_root.exists():
        for old_dir in registry_root.iterdir():
            if not old_dir.is_dir() or old_dir.name == slug:
                continue
            old_meta = old_dir / "meta.json"
            try:
                old_data = json.loads(old_meta.read_text())
                if old_data.get("path") == str(repo_path):
                    shutil.rmtree(old_dir, ignore_errors=True)
            except (json.JSONDecodeError, OSError):
                continue

    slot = registry_root / slug
    slot.mkdir(parents=True, exist_ok=True)

    registry_meta = dict(meta)
    registry_meta["slug"] = slug
    (slot / "meta.json").write_text(
        json.dumps(registry_meta, indent=2) + "\n", encoding="utf-8"
    )


app = typer.Typer(
    name="axon",
    help="Axon — Graph-powered code intelligence engine.",
    no_args_is_help=True,
)

daemon_app = typer.Typer(help="Manage the axon background daemon.")
app.add_typer(daemon_app, name="daemon")


@daemon_app.command("start")
def daemon_start(
    max_dbs: int = typer.Option(5, "--max-dbs", help="Max cached KuzuDB backends (default: 5)"),
) -> None:
    """Start the axon daemon in the background."""
    from axon.core.paths import daemon_pid_path, daemon_sock_path

    pid_path = daemon_pid_path()
    sock_path = daemon_sock_path()

    # Check if already running
    if pid_path.exists():
        try:
            pid = int(pid_path.read_text().strip())
            os.kill(pid, 0)  # Raises ProcessLookupError if dead
            console.print(f"[yellow]Daemon already running[/yellow] (PID {pid})")
            return
        except (ValueError, ProcessLookupError):
            pid_path.unlink(missing_ok=True)  # Stale PID

    # Remove stale socket
    sock_path.unlink(missing_ok=True)

    # Spawn daemon subprocess (detached)
    proc = subprocess.Popen(
        [sys.executable, "-m", "axon.daemon", "--max-dbs", str(max_dbs)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )

    # Wait for socket to appear (up to 5 s)
    for _ in range(50):
        if sock_path.exists():
            console.print(f"[green]Daemon started[/green] (PID {proc.pid})")
            return
        time.sleep(0.1)

    console.print(f"[red]Daemon failed to start[/red] (socket not created in 5 s, PID {proc.pid})")
    raise typer.Exit(code=1)


@daemon_app.command("stop")
def daemon_stop() -> None:
    """Stop the running axon daemon."""
    from axon.core.paths import daemon_pid_path

    pid_path = daemon_pid_path()
    if not pid_path.exists():
        console.print("[yellow]No daemon running[/yellow] (no PID file found)")
        return

    try:
        pid = int(pid_path.read_text().strip())
    except ValueError:
        console.print("[red]Error:[/red] Invalid PID file")
        pid_path.unlink(missing_ok=True)
        raise typer.Exit(code=1)

    try:
        os.kill(pid, signal.SIGTERM)
        console.print(f"[green]Daemon stopped[/green] (PID {pid})")
    except ProcessLookupError:
        console.print(f"[yellow]Daemon was not running[/yellow] (stale PID {pid})")
        pid_path.unlink(missing_ok=True)


@daemon_app.command("status")
def daemon_status() -> None:
    """Show axon daemon status and cached repos."""
    from axon.core.paths import daemon_pid_path, daemon_sock_path
    from axon.daemon.protocol import decode_request, encode_request

    pid_path = daemon_pid_path()
    sock_path = daemon_sock_path()

    if not pid_path.exists():
        console.print("[yellow]Daemon not running[/yellow] (no PID file)")
        return

    try:
        pid = int(pid_path.read_text().strip())
        os.kill(pid, 0)
    except (ValueError, ProcessLookupError):
        console.print("[yellow]Daemon not running[/yellow] (stale PID file)")
        pid_path.unlink(missing_ok=True)
        return

    console.print(f"[green]Daemon running[/green] (PID {pid})")

    # Query daemon for cache stats via socket (best-effort)
    if not sock_path.exists():
        console.print("[yellow]Socket not found[/yellow] — daemon may be starting")
        return

    try:
        import socket as _socket
        with _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM) as sock:
            sock.settimeout(2.0)
            sock.connect(str(sock_path))
            sock.sendall(encode_request("axon_daemon_status", {}, request_id="status"))
            data = b""
            while not data.endswith(b"\n"):
                chunk = sock.recv(4096)
                if not chunk:
                    break
                data += chunk
        resp = decode_request(data)
        if resp.get("result"):
            console.print(resp["result"])
    except Exception as exc:  # noqa: BLE001
        console.print(f"[yellow]Could not query daemon:[/yellow] {exc}")


def _version_callback(value: bool) -> None:
    """Print the version and exit."""
    if value:
        console.print(f"Axon v{__version__}")
        raise typer.Exit()

@app.callback()
def main(
    version: Optional[bool] = typer.Option(  # noqa: N803
        None,
        "--version",
        "-v",
        help="Show version and exit.",
        callback=_version_callback,
        is_eager=True,
    ),
) -> None:
    """Axon — Graph-powered code intelligence engine."""

@app.command()
def analyze(
    path: Path = typer.Argument(Path("."), help="Path to the repository to index."),
    full: bool = typer.Option(False, "--full", help="Perform a full re-index."),
    no_embeddings: bool = typer.Option(
        False, "--no-embeddings", help="Skip vector embedding generation."
    ),
    show_progress: bool = typer.Option(
        False, "--progress", help="Print each completed phase to stderr during indexing."
    ),
) -> None:
    """Index a repository into a knowledge graph."""
    from axon.core.ingestion.pipeline import PipelineResult, run_pipeline
    from axon.core.storage.kuzu_backend import KuzuBackend

    repo_path = path.resolve()
    if not repo_path.is_dir():
        console.print(f"[red]Error:[/red] {repo_path} is not a directory.")
        raise typer.Exit(code=1)

    console.print(f"[bold]Indexing[/bold] {repo_path}")

    axon_dir = repo_path / ".axon"
    axon_dir.mkdir(parents=True, exist_ok=True)

    slug = compute_repo_slug(repo_path)

    # Auto-migrate existing local DB to central location
    _auto_migrate_local_kuzu(repo_path, slug)

    # Central DB path
    central_slot = Path.home() / ".axon" / "repos" / slug
    central_slot.mkdir(parents=True, exist_ok=True)
    # Write a placeholder meta.json to the central slot so _register_in_global_registry
    # doesn't treat it as a corrupt entry and delete the directory (and the kuzu DB).
    central_placeholder = central_slot / "meta.json"
    if not central_placeholder.exists():
        central_placeholder.write_text(
            json.dumps({"path": str(repo_path), "name": repo_path.name}) + "\n",
            encoding="utf-8",
        )
    db_path = _central_db_path(slug)

    # Singleton lock — prevents two concurrent `axon analyze` on the same repo.
    # fcntl.flock is released automatically on process death (SIGKILL/OOM), so
    # there is no risk of stale locks blocking future runs.
    lock_path = central_slot / "analyze.lock"
    lock_fd = open(lock_path, "w")  # noqa: SIM115
    try:
        fcntl.flock(lock_fd.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock_fd.close()
        console.print(
            f"[red]Error:[/red] '{slug}' is already being indexed by another process.\n"
            "Wait for it to complete, or kill the blocking process."
        )
        raise typer.Exit(code=1)

    try:
        # Auto-detect stale index: files indexed but 0 symbols → force full re-index.
        if not full:
            meta_path = axon_dir / "meta.json"
            if meta_path.exists():
                try:
                    prev = json.loads(meta_path.read_text(encoding="utf-8"))
                    prev_stats = prev.get("stats", {})
                    if prev_stats.get("files", 0) > 0 and prev_stats.get("symbols", 0) == 0:
                        full = True
                        console.print(
                            "[yellow]Warning: previous index has no symbols"
                            " — forcing full re-index.[/yellow]"
                        )
                except (json.JSONDecodeError, KeyError):
                    pass

        storage = KuzuBackend()
        storage.initialize(db_path)

        result: PipelineResult | None = None
        with Progress(
            SpinnerColumn(),
            TextColumn("[progress.description]{task.description}"),
            console=console,
            transient=True,
        ) as progress:
            task = progress.add_task("Starting...", total=None)

            _show_progress = show_progress or bool(os.getenv("AXON_ANALYZE_PROGRESS"))

            def on_progress(phase: str, pct: float) -> None:
                progress.update(task, description=f"{phase} ({pct:.0%})")
                if _show_progress and pct >= 1.0:
                    print(f"[{phase}] done", file=sys.stderr, flush=True)

            _, result = run_pipeline(
                repo_path=repo_path,
                storage=storage,
                full=full,
                progress_callback=on_progress,
                embeddings=not no_embeddings,
                wait_embeddings=True,
            )

        meta = {
            "version": __version__,
            "name": repo_path.name,
            "slug": slug,
            "path": str(repo_path),
            "stats": {
                "files": result.files,
                "symbols": result.symbols,
                "relationships": result.relationships,
                "clusters": result.clusters,
                "flows": result.processes,
                "dead_code": result.dead_code,
                "coupled_pairs": result.coupled_pairs,
                "embeddings": result.embeddings,
            },
            "last_indexed_at": datetime.now(tz=timezone.utc).isoformat(),
        }
        meta_path = axon_dir / "meta.json"
        meta_content = json.dumps(meta, indent=2) + "\n"
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=axon_dir,
            delete=False,
            suffix=".tmp",
        ) as tmp_f:
            tmp_f.write(meta_content)
            tmp_name = tmp_f.name
        os.replace(tmp_name, meta_path)

        try:
            _register_in_global_registry(meta, repo_path)
        except (OSError, ValueError, KeyError):
            logger.debug("Failed to register repo in global registry", exc_info=True)

        console.print()
        console.print("[bold green]Indexing complete.[/bold green]")
        console.print(f"  Files:          {result.files}")
        console.print(f"  Symbols:        {result.symbols}")
        console.print(f"  Relationships:  {result.relationships}")
        if result.clusters > 0:
            console.print(f"  Clusters:       {result.clusters}")
        if result.processes > 0:
            console.print(f"  Flows:          {result.processes}")
        if result.dead_code > 0:
            console.print(f"  Dead code:      {result.dead_code}")
        if result.coupled_pairs > 0:
            console.print(f"  Coupled pairs:  {result.coupled_pairs}")
        if result.embeddings > 0:
            console.print(f"  Embeddings:     {result.embeddings}")
        console.print(f"  Duration:       {result.duration_seconds:.2f}s")

        storage.close()
    finally:
        fcntl.flock(lock_fd.fileno(), fcntl.LOCK_UN)
        lock_fd.close()

@app.command()
def status() -> None:
    """Show index status for current repository."""
    repo_path = Path.cwd().resolve()
    meta_path = repo_path / ".axon" / "meta.json"

    if not meta_path.exists():
        console.print(
            f"[red]Error:[/red] No index found at {repo_path}. Run 'axon analyze' first."
        )
        raise typer.Exit(code=1)

    meta = json.loads(meta_path.read_text(encoding="utf-8"))
    stats = meta.get("stats", {})

    console.print(f"[bold]Index status for[/bold] {repo_path}")
    console.print(f"  Version:        {meta.get('version', '?')}")
    console.print(f"  Last indexed:   {meta.get('last_indexed_at', '?')}")
    console.print(f"  Files:          {stats.get('files', '?')}")
    console.print(f"  Symbols:        {stats.get('symbols', '?')}")
    console.print(f"  Relationships:  {stats.get('relationships', '?')}")

    if stats.get("clusters", 0) > 0:
        console.print(f"  Clusters:       {stats['clusters']}")
    if stats.get("flows", 0) > 0:
        console.print(f"  Flows:          {stats['flows']}")
    if stats.get("dead_code", 0) > 0:
        console.print(f"  Dead code:      {stats['dead_code']}")
    if stats.get("coupled_pairs", 0) > 0:
        console.print(f"  Coupled pairs:  {stats['coupled_pairs']}")

@app.command(name="list")
def list_repos() -> None:
    """List all indexed repositories."""
    from axon.mcp.tools import handle_list_repos

    result = handle_list_repos()
    console.print(result)

@app.command()
def clean(
    force: bool = typer.Option(False, "--force", "-f", help="Skip confirmation prompt."),
) -> None:
    """Delete index for current repository."""
    repo_path = Path.cwd().resolve()
    axon_dir = repo_path / ".axon"

    if not axon_dir.exists():
        console.print(
            f"[red]Error:[/red] No index found at {repo_path}. Nothing to clean."
        )
        raise typer.Exit(code=1)

    if not force:
        confirm = typer.confirm(f"Delete index at {axon_dir}?")
        if not confirm:
            console.print("Aborted.")
            raise typer.Exit()

    # Delete central DB if slug is known
    meta_path = axon_dir / "meta.json"
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text(encoding="utf-8"))
            slug = meta.get("slug")
            if slug:
                central_dir = Path.home() / ".axon" / "repos" / slug
                if central_dir.exists():
                    shutil.rmtree(central_dir)
                    console.print(f"[green]Deleted[/green] central DB {central_dir}")
        except (json.JSONDecodeError, OSError):
            pass

    shutil.rmtree(axon_dir)
    console.print(f"[green]Deleted[/green] {axon_dir}")

@app.command()
def query(
    q: str = typer.Argument(..., help="Search query for the knowledge graph."),
    limit: int = typer.Option(20, "--limit", "-n", help="Maximum number of results."),
) -> None:
    """Search the knowledge graph."""
    from axon.mcp.tools import handle_query

    storage = _load_storage()
    result = handle_query(storage, q, limit=limit)
    console.print(result)
    storage.close()

@app.command()
def context(
    name: str = typer.Argument(..., help="Symbol name to inspect."),
) -> None:
    """Show 360-degree view of a symbol."""
    from axon.mcp.tools import handle_context

    storage = _load_storage()
    result = handle_context(storage, name)
    console.print(result)
    storage.close()

@app.command()
def impact(
    target: str = typer.Argument(..., help="Symbol to analyze blast radius for."),
    depth: int = typer.Option(3, "--depth", "-d", min=1, max=10, help="Traversal depth (1-10)."),
) -> None:
    """Show blast radius of changing a symbol."""
    from axon.mcp.tools import handle_impact

    storage = _load_storage()
    result = handle_impact(storage, target, depth=depth)
    console.print(result)
    storage.close()

@app.command(name="dead-code")
def dead_code(
    exit_code: bool = typer.Option(
        False, "--exit-code", help="Exit 1 if dead code found (for CI)."
    ),
) -> None:
    """List all detected dead code."""
    from axon.mcp.tools import handle_dead_code

    storage = _load_storage()
    result = handle_dead_code(storage)
    console.print(result)
    storage.close()
    if exit_code and not result.startswith("No dead code"):
        raise typer.Exit(code=1)

@app.command()
def cypher(
    query: str = typer.Argument(..., help="Raw Cypher query to execute."),
) -> None:
    """Execute raw Cypher against the knowledge graph."""
    from axon.mcp.tools import handle_cypher

    storage = _load_storage()
    result = handle_cypher(storage, query)
    console.print(result)
    storage.close()

@app.command()
def setup(
    claude: bool = typer.Option(False, "--claude", help="Configure MCP for Claude Code."),
    cursor: bool = typer.Option(False, "--cursor", help="Configure MCP for Cursor."),
) -> None:
    """Configure MCP for Claude Code / Cursor."""
    mcp_config = {
        "command": "axon",
        "args": ["serve", "--watch"],
    }

    if claude or (not claude and not cursor):
        console.print("[bold]Add to your Claude Code MCP config:[/bold]")
        console.print(json.dumps({"axon": mcp_config}, indent=2))

    if cursor or (not claude and not cursor):
        console.print("[bold]Add to your Cursor MCP config:[/bold]")
        console.print(json.dumps({"axon": mcp_config}, indent=2))

@app.command()
def watch(
    debounce: int = typer.Option(500, "--debounce", help="Debounce interval in ms (default: 500)."),
) -> None:
    """Watch mode — re-index on file changes."""
    import asyncio

    from axon.core.ingestion.pipeline import run_pipeline
    from axon.core.ingestion.watcher import watch_repo
    from axon.core.storage.kuzu_backend import KuzuBackend

    repo_path = Path.cwd().resolve()
    axon_dir = repo_path / ".axon"
    axon_dir.mkdir(parents=True, exist_ok=True)

    slug = compute_repo_slug(repo_path)
    _auto_migrate_local_kuzu(repo_path, slug)
    (Path.home() / ".axon" / "repos" / slug).mkdir(parents=True, exist_ok=True)
    db_path = _central_db_path(slug)

    storage = KuzuBackend()
    storage.initialize(db_path)

    if not (axon_dir / "meta.json").exists():
        console.print("[bold]Running initial index...[/bold]")
        run_pipeline(repo_path, storage, full=True)

    console.print(f"[bold]Watching[/bold] {repo_path} for changes (Ctrl+C to stop)")

    try:
        asyncio.run(watch_repo(repo_path, storage, debounce_ms=debounce))
    except KeyboardInterrupt:
        console.print("\n[bold]Watch stopped.[/bold]")
    finally:
        storage.close()

@app.command()
def diff(
    branch_range: str = typer.Argument(
        ..., help="Branch range for comparison (e.g. main..feature)."
    ),
) -> None:
    """Structural branch comparison."""
    from axon.core.diff import diff_branches, format_diff

    repo_path = Path.cwd().resolve()
    try:
        result = diff_branches(repo_path, branch_range)
    except (ValueError, RuntimeError) as exc:
        console.print(f"[red]Error:[/red] {exc}")
        raise typer.Exit(code=1) from exc

    console.print(format_diff(result))

@app.command()
def mcp() -> None:
    """Start MCP server (stdio transport)."""
    import asyncio

    from axon.mcp.server import main as mcp_main

    asyncio.run(mcp_main())

_SHELL_HOOK_BASH = """\
# Axon shell integration
# Add to ~/.bashrc:  eval "$(axon shell-hook)"
_axon_chpwd() {
  if [[ -d ".axon" ]] && command -v axon >/dev/null 2>&1; then
    local pid_file=".axon/watch.pid"
    if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      return
    fi
    axon watch >/dev/null 2>&1 &
    echo $! > "$pid_file"
  fi
}
PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }_axon_chpwd"
"""

_SHELL_HOOK_ZSH = """\
# Axon shell integration
# Add to ~/.zshrc:  eval "$(axon shell-hook --shell zsh)"
_axon_chpwd() {
  if [[ -d ".axon" ]] && command -v axon >/dev/null 2>&1; then
    local pid_file=".axon/watch.pid"
    if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      return
    fi
    axon watch >/dev/null 2>&1 &
    echo $! > "$pid_file"
  fi
}
autoload -U add-zsh-hook
add-zsh-hook chpwd _axon_chpwd
"""

_ENVRC_SENTINEL = "# >>> axon auto-start <<<"

_ENVRC_BLOCK = """\
# >>> axon auto-start <<<
if command -v axon >/dev/null 2>&1 && [[ -d ".axon" ]]; then
  _axon_pid_file=".axon/watch.pid"
  if ! { [[ -f "$_axon_pid_file" ]] && kill -0 "$(cat "$_axon_pid_file")" 2>/dev/null; }; then
    axon watch >/dev/null 2>&1 &
    echo $! > "$_axon_pid_file"
  fi
fi
# <<< axon auto-start <<<
"""


@app.command(name="shell-hook")
def shell_hook(
    shell: str = typer.Option("bash", "--shell", "-s", help="Shell type: bash or zsh."),
) -> None:
    """Print shell integration code to auto-start axon watcher on cd."""
    if shell == "bash":
        print(_SHELL_HOOK_BASH, end="")
    elif shell == "zsh":
        print(_SHELL_HOOK_ZSH, end="")
    else:
        console.print(f"[red]Error:[/red] Unsupported shell '{shell}'. Use 'bash' or 'zsh'.")
        raise typer.Exit(code=1)


@app.command()
def init(
    direnv: bool = typer.Option(False, "--direnv", help="Create/update .envrc with axon auto-start."),  # noqa: E501
) -> None:
    """Initialize axon shell integration for the current project."""
    if not direnv:
        console.print("[bold]Axon Shell Integration[/bold]")
        console.print()
        console.print("[bold]Option 1 — Shell hook (bash/zsh)[/bold]")
        console.print("Add to ~/.bashrc or ~/.zshrc:")
        console.print()
        console.print('    eval "$(axon shell-hook)"              # bash (default)')
        console.print('    eval "$(axon shell-hook --shell zsh)"  # zsh')
        console.print()
        console.print("[bold]Option 2 — direnv[/bold]")
        console.print("Run in your project directory:")
        console.print()
        console.print("    axon init --direnv")
        if shutil.which("direnv"):
            console.print("    direnv allow")
        console.print()
        console.print(
            "Both methods auto-start [bold]axon watch[/bold] when you cd into a project with .axon/."  # noqa: E501
        )
        return

    envrc_path = Path.cwd() / ".envrc"

    if envrc_path.exists():
        existing = envrc_path.read_text(encoding="utf-8")
        if _ENVRC_SENTINEL in existing:
            console.print("Axon block already in .envrc — skipping.")
            return
        updated = existing.rstrip("\n") + "\n\n" + _ENVRC_BLOCK
        envrc_path.write_text(updated, encoding="utf-8")
        console.print("[green]Appended[/green] axon auto-start block to .envrc")
    else:
        envrc_path.write_text(_ENVRC_BLOCK, encoding="utf-8")
        console.print("[green]Created[/green] .envrc with axon auto-start block")

    if shutil.which("direnv"):
        console.print("Run [bold]direnv allow[/bold] to activate.")


@app.command()
def stats() -> None:
    """Show axon usage statistics from event log."""
    from collections import Counter

    events_path = Path.home() / ".axon" / "events.jsonl"
    if not events_path.exists():
        console.print(
            "No usage data found. Run 'axon analyze' or 'axon query' to generate stats."
        )
        return

    events: list[dict] = []
    with events_path.open(encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue

    if not events:
        console.print("No usage data found.")
        return

    query_events = [e for e in events if e.get("type") in {"query", "context", "impact"}]
    index_events = [e for e in events if e.get("type") == "index"]

    console.print("[bold]Axon Usage Statistics[/bold]")
    console.print()
    console.print(f"  Queries:        {len(query_events)}")
    query_texts = [e["query"] for e in query_events if e.get("query")]
    console.print(f"  Unique queries: {len(set(query_texts))}")
    console.print(f"  Index runs:     {len(index_events)}")

    if query_texts:
        console.print()
        console.print("[bold]Top 5 queries:[/bold]")
        for text, count in Counter(query_texts).most_common(5):
            console.print(f"  {count:>3}x  {text!r}")

    if index_events:
        console.print()
        console.print("[bold]Per-repo index activity:[/bold]")
        repo_events: dict[str, list[dict]] = {}
        for e in index_events:
            repo_name = e.get("repo", "unknown")
            repo_events.setdefault(repo_name, []).append(e)
        for repo_name, revents in sorted(repo_events.items()):
            last_ts = max(e.get("ts", "") for e in revents)
            console.print(f"  {repo_name:<30} {len(revents):>3} run(s), last: {last_ts}")


@app.command()
def serve(
    watch: bool = typer.Option(
        False, "--watch", "-w", help="Enable file watching with auto-reindex."
    ),
    debounce: int = typer.Option(500, "--debounce", help="Debounce interval in ms (default: 500)."),
) -> None:
    """Start MCP server, optionally with live file watching."""
    import asyncio
    import sys

    from axon.mcp.server import main as mcp_main
    from axon.mcp.server import set_lock, set_storage

    if not watch:
        asyncio.run(mcp_main())
        return

    from axon.core.ingestion.pipeline import run_pipeline
    from axon.core.ingestion.watcher import watch_repo
    from axon.core.storage.kuzu_backend import KuzuBackend

    repo_path = Path.cwd().resolve()
    axon_dir = repo_path / ".axon"
    axon_dir.mkdir(parents=True, exist_ok=True)

    slug = compute_repo_slug(repo_path)
    _auto_migrate_local_kuzu(repo_path, slug)
    (Path.home() / ".axon" / "repos" / slug).mkdir(parents=True, exist_ok=True)
    db_path = _central_db_path(slug)

    storage = KuzuBackend()
    storage.initialize(db_path)

    if not (axon_dir / "meta.json").exists():
        print("Running initial index...", file=sys.stderr)
        run_pipeline(repo_path, storage, full=True)

    lock = asyncio.Lock()
    set_storage(storage)
    set_lock(lock)

    async def _run() -> None:
        from mcp.server.stdio import stdio_server

        from axon.mcp.server import server as mcp_server

        stop = asyncio.Event()

        async with stdio_server() as (read, write):
            async def _mcp_then_stop():
                await mcp_server.run(read, write, mcp_server.create_initialization_options())
                stop.set()

            await asyncio.gather(
                _mcp_then_stop(),
                watch_repo(repo_path, storage, stop_event=stop, lock=lock, debounce_ms=debounce),
            )

    try:
        asyncio.run(_run())
    except KeyboardInterrupt:
        pass
    finally:
        storage.close()
