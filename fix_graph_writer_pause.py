with open("src/axon-core/src/graph_writer.rs", "r") as f:
    content = f.read()

# Add pause_ingestion parameter to function signature
content = content.replace("pub fn spawn_graph_writer(store: Arc<std::sync::RwLock<GraphStore>>) -> UnboundedSender<GraphWriteTask> {",
                          "pub fn spawn_graph_writer(\n    store: Arc<std::sync::RwLock<GraphStore>>,\n    pause_ingestion: Arc<std::sync::atomic::AtomicBool>,\n) -> UnboundedSender<GraphWriteTask> {")

# Add the check loop logic
content = content.replace("        loop {\n            tokio::select! {\n                _ = interval.tick() => {",
                          "        loop {\n            if pause_ingestion.load(std::sync::atomic::Ordering::Relaxed) {\n                tokio::time::sleep(Duration::from_millis(100)).await;\n                continue;\n            }\n\n            tokio::select! {\n                _ = interval.tick() => {")

with open("src/axon-core/src/graph_writer.rs", "w") as f:
    f.write(content)
