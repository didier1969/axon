with open("src/axon-core/src/main.rs", "r") as f:
    content = f.read()

# Add pause_ingestion atomic bool in main.rs
content = content.replace(
    "let parse_semaphore = Arc::new(Semaphore::new(16));\n    \n    // We wrap GraphStore",
    "let parse_semaphore = Arc::new(Semaphore::new(16));\n    let pause_ingestion = Arc::new(std::sync::atomic::AtomicBool::new(false));\n    \n    // We wrap GraphStore"
)

# Pass it to spawn_graph_writer
content = content.replace(
    "let batch_tx = graph_writer::spawn_graph_writer(graph_store.clone());",
    "let batch_tx = graph_writer::spawn_graph_writer(graph_store.clone(), pause_ingestion.clone());"
)

# Pass it to start_listener
content = content.replace(
    "graph_store,\n        batch_tx,\n        parse_semaphore,\n    ).await?;",
    "graph_store,\n        batch_tx,\n        parse_semaphore,\n        pause_ingestion,\n    ).await?;"
)

with open("src/axon-core/src/main.rs", "w") as f:
    f.write(content)
