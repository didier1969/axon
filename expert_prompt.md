Analyze the following architectural bottleneck in an Elixir/Rust graph database ingestion pipeline and propose the state-of-the-art solution.

CONTEXT:
- System: Axon V2. A code analysis tool.
- Control Plane: Elixir (Watcher/UI). Discovers files via `inotify` or full scans.
- Data Plane: Rust (Parser + KuzuDB). Parses AST via Tree-sitter, generates embeddings via FastEmbed, and inserts nodes/edges into an embedded graph DB (KuzuDB).
- Communication: Unix Domain Socket (JSON-RPC) between Elixir and Rust.
- Current State: When Elixir discovers 36,000 files, it dumps them into an Oban queue. Elixir workers send `PARSE_FILE` JSON requests to Rust as fast as possible. Rust creates a `tokio::spawn_blocking` task for each file, does heavy CPU work (WASM + Neural Net), then acquires a global `RwLock::write` on KuzuDB to insert the graph nodes.
- Problem: The LLM agents query the system via the SAME Rust process (MCP protocol). Because Rust is bombarded with 36,000 ingestion requests, the CPU threads starve, and the KuzuDB Write Lock is constantly held. LLM read requests time out.

USER'S PROPOSAL:
"We should manage a priority queue of these 36,000 files. We ingest them progressively (drip-feed). We prioritize files recently modified. When a specific read request comes in, we pause the ingestion, serve the read request, and resume. We check every minute for new files instead of doing a massive dump."

TASK:
As 3 different expert personas, evaluate the user's proposal and provide the absolute best state-of-the-art implementation strategy for this specific Elixir/Rust architecture. Be honest, critical, and specific.

EXPERT 1: Lead Elixir/OTP Architect (Focuses on Oban, Backpressure, and GenStage/Broadway).
EXPERT 2: Lead Rust/Systems Engineer (Focuses on Tokio, Lock-free data structures, Batching, and CPU isolation).
EXPERT 3: Lead Database/Graph Engineer (Focuses on KuzuDB write/read contention, MVCC, and transaction batching).
