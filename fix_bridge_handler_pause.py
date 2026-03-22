with open("src/axon-core/src/bridge_handler.rs", "r") as f:
    content = f.read()

# Add pause_ingestion parameter to function signature
content = content.replace("pub async fn handle_command(\n    command: String,\n    tx: Sender<String>,\n    store_clone: Arc<std::sync::RwLock<GraphStore>>,\n    batch_tx_clone: UnboundedSender<GraphWriteTask>,\n    parse_sem_clone: Arc<Semaphore>,\n    cancel_token: &mut Arc<AtomicBool>,\n    scan_task: &mut Option<tokio::task::JoinHandle<()>>,\n    projects_root: &str,\n) {",
                          "pub async fn handle_command(\n    command: String,\n    tx: Sender<String>,\n    store_clone: Arc<std::sync::RwLock<GraphStore>>,\n    batch_tx_clone: UnboundedSender<GraphWriteTask>,\n    parse_sem_clone: Arc<Semaphore>,\n    cancel_token: &mut Arc<AtomicBool>,\n    scan_task: &mut Option<tokio::task::JoinHandle<()>>,\n    projects_root: &str,\n    pause_ingestion: Arc<std::sync::atomic::AtomicBool>,\n) {")

# Update MCP request branch to set pause flag
mcp_branch = """    } else if command.starts_with("{") {
        let store_for_mcp = store_clone.clone();
        let command_clone = command.to_string();
        let tx_clone = tx.clone();
        let pause_flag = pause_ingestion.clone();
        
        tokio::spawn(async move {
            // Priority execution: Pause background indexing
            pause_flag.store(true, Ordering::Relaxed);
            
            // Wait briefly to let currently running background flush complete
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            
            let mcp_server = McpServer::new(store_for_mcp);
            if let Ok(request) = serde_json::from_str::<mcp::JsonRpcRequest>(&command_clone) {
                let response = tokio::task::spawn_blocking(move || {
                    mcp_server.handle_request(request)
                }).await.expect("Blocking MCP task panicked");
                
                if let Ok(json_str) = serde_json::to_string(&response) {
                    let _ = tx_clone.send(format!("{}\\n", json_str)).await;
                }
            }
            
            // Resume background indexing
            pause_flag.store(false, Ordering::Relaxed);
        });
    }"""

# We replace the original MCP block
import re
content = re.sub(r'\} else if command\.starts_with\("\{"\) \{\s*let store_for_mcp.*?\n    \}\s*\}$', mcp_branch + "\n}", content, flags=re.DOTALL)

with open("src/axon-core/src/bridge_handler.rs", "w") as f:
    f.write(content)
