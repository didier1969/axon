// REQ-AXO-902679 — Native Apache Arrow Flight SQL Streaming Server.
//
// Exposes high-throughput IPC streaming of symbols and graph edges over local
// Unix Domain Sockets (/tmp/axon_flight.sock) and TCP (127.0.0.1:44150).
// Eliminates JSON boxing and supports zero-copy ingestion by external planes.

use std::fs;
use std::io::Cursor;
use std::net::SocketAddr;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::RecordBatch;
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::watch;

use crate::arrow_flight::flight_stream::FlightSqlStreamBridge;
use crate::arrow_flight::record_batch::{EDGES_SCHEMA, SYMBOLS_SCHEMA};
use crate::ist_snapshot::cache::IstSnapshotCache;
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeKind, NodeRecord};

/// Abstraction allowing FlightServer to serve symbols and edges from in-memory
/// snapshot graphs or persistent stores without coupling.
pub trait FlightDataProvider: Send + Sync + 'static {
    fn get_nodes(&self, project: Option<&str>) -> Vec<NodeRecord>;
    fn get_edges(&self, project: Option<&str>) -> Vec<EdgeTriple>;
}

impl FlightDataProvider for IstSnapshotCache {
    fn get_nodes(&self, project: Option<&str>) -> Vec<NodeRecord> {
        let projects = match project {
            Some(p) => vec![p.to_string()],
            None => self.project_codes(),
        };
        let mut nodes = Vec::new();
        for p in projects {
            if let Some(graph) = self.get(&p) {
                for i in 0..graph.node_count() {
                    let idx = i as u32;
                    let (kind_byte, proj, flags) = graph.node_meta(idx);
                    nodes.push(NodeRecord {
                        id: graph.id_of(idx).to_string(),
                        name: graph.name_of(idx).to_string(),
                        project_code: proj.to_string(),
                        kind: NodeKind::from_u8(kind_byte),
                        flags,
                        complexity: graph.complexity_of(idx),
                    });
                }
            }
        }
        nodes
    }

    fn get_edges(&self, project: Option<&str>) -> Vec<EdgeTriple> {
        let projects = match project {
            Some(p) => vec![p.to_string()],
            None => self.project_codes(),
        };
        let mut edges = Vec::new();
        for p in projects {
            if let Some(graph) = self.get(&p) {
                for i in 0..graph.node_count() {
                    let idx = i as u32;
                    let src_id = graph.id_of(idx);
                    for (tgt_idx, rel) in graph.forward_neighbors(idx) {
                        edges.push(EdgeTriple {
                            source: src_id.to_string(),
                            target: graph.id_of(tgt_idx).to_string(),
                            rel,
                        });
                    }
                }
            }
        }
        edges
    }
}

/// Commands accepted by the lightweight Flight SQL streaming protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum FlightCommand {
    Symbols {
        project: Option<String>,
        batch_size: Option<usize>,
    },
    Edges {
        project: Option<String>,
        batch_size: Option<usize>,
    },
    Ping,
}

/// Configuration parameters for FlightServer socket endpoints.
#[derive(Debug, Clone)]
pub struct FlightServerConfig {
    pub unix_socket_path: Option<PathBuf>,
    pub tcp_bind_addr: Option<SocketAddr>,
    pub default_batch_size: usize,
}

impl Default for FlightServerConfig {
    fn default() -> Self {
        Self {
            unix_socket_path: Some(PathBuf::from("/tmp/axon_flight.sock")),
            tcp_bind_addr: Some("127.0.0.1:44150".parse().unwrap()),
            default_batch_size: 1000,
        }
    }
}

/// Native Apache Arrow IPC streaming server.
pub struct FlightServer<P: FlightDataProvider> {
    config: FlightServerConfig,
    provider: Arc<P>,
    tcp_listener: Option<TcpListener>,
    unix_listener: Option<UnixListener>,
}

impl<P: FlightDataProvider> FlightServer<P> {
    /// Creates and immediately binds the configured network listeners.
    pub fn new(config: FlightServerConfig, provider: Arc<P>) -> Self {
        let tcp_listener = config.tcp_bind_addr.and_then(|addr| {
            let std_listener = std::net::TcpListener::bind(addr).ok()?;
            std_listener.set_nonblocking(true).ok()?;
            TcpListener::from_std(std_listener).ok()
        });

        let unix_listener = config.unix_socket_path.as_ref().and_then(|path| {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
            let std_listener = StdUnixListener::bind(path).ok()?;
            std_listener.set_nonblocking(true).ok()?;
            UnixListener::from_std(std_listener).ok()
        });

        Self {
            config,
            provider,
            tcp_listener,
            unix_listener,
        }
    }

    /// Returns the local address of the bound TCP listener, if enabled.
    pub fn tcp_local_addr(&self) -> Option<SocketAddr> {
        self.tcp_listener.as_ref().and_then(|l| l.local_addr().ok())
    }

    /// Runs the server event loop until a shutdown signal is received.
    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        log::info!(
            "Arrow Flight streaming server listening on unix={:?}, tcp={:?}",
            self.config.unix_socket_path,
            self.tcp_local_addr()
        );

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        log::info!("Flight server received shutdown signal, exiting cleanly");
                        break;
                    }
                }
                res = async {
                    match self.tcp_listener.as_ref() {
                        Some(listener) => listener.accept().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Ok((stream, peer)) = res {
                        let provider = Arc::clone(&self.provider);
                        let default_batch = self.config.default_batch_size;
                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_tcp_client(stream, provider, default_batch).await {
                                log::debug!("Flight TCP client error from {}: {:?}", peer, e);
                            }
                        });
                    }
                }
                res = async {
                    match self.unix_listener.as_ref() {
                        Some(listener) => listener.accept().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Ok((stream, _)) = res {
                        let provider = Arc::clone(&self.provider);
                        let default_batch = self.config.default_batch_size;
                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_unix_client(stream, provider, default_batch).await {
                                log::debug!("Flight Unix client error: {:?}", e);
                            }
                        });
                    }
                }
            }
        }

        // Clean up unix socket file on graceful shutdown
        if let Some(path) = &self.config.unix_socket_path {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }

        Ok(())
    }

    async fn handle_tcp_client(
        stream: TcpStream,
        provider: Arc<P>,
        default_batch_size: usize,
    ) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;

        let response_bytes = Self::dispatch_command(&line, &provider, default_batch_size).await?;
        writer.write_all(&response_bytes).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn handle_unix_client(
        stream: UnixStream,
        provider: Arc<P>,
        default_batch_size: usize,
    ) -> Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;

        let response_bytes = Self::dispatch_command(&line, &provider, default_batch_size).await?;
        writer.write_all(&response_bytes).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn dispatch_command(
        line: &str,
        provider: &Arc<P>,
        default_batch_size: usize,
    ) -> Result<Vec<u8>> {
        let trimmed = line.trim();
        if trimmed == "ping" || trimmed == "{\"command\":\"ping\"}" {
            return Ok(b"pong\n".to_vec());
        }

        let cmd: FlightCommand = serde_json::from_str(trimmed)
            .context("failed to deserialize FlightCommand JSON payload")?;

        match cmd {
            FlightCommand::Ping => Ok(b"pong\n".to_vec()),
            FlightCommand::Symbols {
                project,
                batch_size,
            } => {
                let nodes = provider.get_nodes(project.as_deref());
                let b_size = batch_size.unwrap_or(default_batch_size);
                let bridge = FlightSqlStreamBridge::new();
                let mut stream = bridge.stream_symbols_batches(nodes, b_size);

                let mut out_buffer = Vec::new();
                {
                    let mut writer = StreamWriter::try_new(&mut out_buffer, &SYMBOLS_SCHEMA)
                        .context("failed to initialize Arrow IPC StreamWriter for symbols")?;

                    while let Some(batch_res) = stream.next().await {
                        let batch = batch_res?;
                        writer.write(&batch)?;
                    }
                    writer.finish()?;
                }
                Ok(out_buffer)
            }
            FlightCommand::Edges {
                project,
                batch_size,
            } => {
                let edges = provider.get_edges(project.as_deref());
                let b_size = batch_size.unwrap_or(default_batch_size);
                let bridge = FlightSqlStreamBridge::new();
                let mut stream = bridge.stream_edges_batches(edges, b_size);

                let mut out_buffer = Vec::new();
                {
                    let mut writer = StreamWriter::try_new(&mut out_buffer, &EDGES_SCHEMA)
                        .context("failed to initialize Arrow IPC StreamWriter for edges")?;

                    while let Some(batch_res) = stream.next().await {
                        let batch = batch_res?;
                        writer.write(&batch)?;
                    }
                    writer.finish()?;
                }
                Ok(out_buffer)
            }
        }
    }
}

/// Client helper for connecting to local or remote FlightServer instances and reading RecordBatches.
pub struct FlightClient;

impl FlightClient {
    /// Connect to Unix domain socket, send command, read and parse Arrow RecordBatches.
    pub async fn query_unix_stream(
        socket_path: &Path,
        cmd: &FlightCommand,
    ) -> Result<Vec<RecordBatch>> {
        let mut stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("failed to connect to unix socket at {:?}", socket_path))?;

        let payload = serde_json::to_string(cmd)? + "\n";
        stream.write_all(payload.as_bytes()).await?;
        stream.flush().await?;

        let mut raw_data = Vec::new();
        stream.read_to_end(&mut raw_data).await?;

        let cursor = Cursor::new(raw_data);
        let reader = StreamReader::try_new(cursor, None)
            .context("failed to create Arrow StreamReader from Unix socket payload")?;

        let mut batches = Vec::new();
        for batch_res in reader {
            batches.push(batch_res?);
        }
        Ok(batches)
    }

    /// Connect to TCP socket, send command, read and parse Arrow RecordBatches.
    pub async fn query_tcp_stream(
        addr: SocketAddr,
        cmd: &FlightCommand,
    ) -> Result<Vec<RecordBatch>> {
        let mut stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("failed to connect to tcp socket at {:?}", addr))?;

        let payload = serde_json::to_string(cmd)? + "\n";
        stream.write_all(payload.as_bytes()).await?;
        stream.flush().await?;

        let mut raw_data = Vec::new();
        stream.read_to_end(&mut raw_data).await?;

        let cursor = Cursor::new(raw_data);
        let reader = StreamReader::try_new(cursor, None)
            .context("failed to create Arrow StreamReader from TCP socket payload")?;

        let mut batches = Vec::new();
        for batch_res in reader {
            batches.push(batch_res?);
        }
        Ok(batches)
    }

    /// Send ping command over TCP socket.
    pub async fn ping_tcp(addr: SocketAddr) -> Result<String> {
        let mut stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("failed to connect to tcp socket at {:?}", addr))?;

        stream.write_all(b"ping\n").await?;
        stream.flush().await?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        Ok(line.trim().to_string())
    }
}

/// Spawns the default Arrow Flight SQL streaming server in the background.
pub fn spawn_flight_server() -> Option<watch::Sender<bool>> {
    let cache = crate::ist_snapshot::shared_cache();
    let config = FlightServerConfig::default();
    let server = FlightServer::new(config, cache);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        if let Err(err) = server.run(shutdown_rx).await {
            log::warn!("Arrow Flight streaming server exited with error: {:?}", err);
        }
    });

    Some(shutdown_tx)
}
