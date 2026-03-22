with open("src/axon-core/src/uds_server.rs", "r") as f:
    content = f.read()

# Add pause_ingestion parameter to start_listener
content = content.replace(
    "pub async fn start_listener(\n    socket_path: &str,\n    boot_time: String,\n    projects_root: String,\n    graph_store: Arc<std::sync::RwLock<GraphStore>>,\n    batch_tx: UnboundedSender<GraphWriteTask>,\n    parse_semaphore: Arc<Semaphore>,\n) -> anyhow::Result<()> {",
    "pub async fn start_listener(\n    socket_path: &str,\n    boot_time: String,\n    projects_root: String,\n    graph_store: Arc<std::sync::RwLock<GraphStore>>,\n    batch_tx: UnboundedSender<GraphWriteTask>,\n    parse_semaphore: Arc<Semaphore>,\n    pause_ingestion: Arc<std::sync::atomic::AtomicBool>,\n) -> anyhow::Result<()> {"
)

# Clone pause_ingestion inside the loop
content = content.replace(
    "let parse_sem_clone = parse_semaphore.clone();\n        let projects_root_clone = projects_root.clone();",
    "let parse_sem_clone = parse_semaphore.clone();\n        let projects_root_clone = projects_root.clone();\n        let pause_ingestion_clone = pause_ingestion.clone();"
)

# Pass it to handle_command
content = content.replace(
    "&mut scan_task,\n                    &projects_root_clone\n                ).await;",
    "&mut scan_task,\n                    &projects_root_clone,\n                    pause_ingestion_clone.clone()\n                ).await;"
)

with open("src/axon-core/src/uds_server.rs", "w") as f:
    f.write(content)
