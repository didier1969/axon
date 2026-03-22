with open("src/axon-core/src/graph_writer.rs", "r") as f:
    content = f.read()

# Make the RwLock block explicit so it gets dropped before the sleep
content = content.replace(
    "flush_buffer(&store, &mut buffer);\n                        tokio::time::sleep(Duration::from_millis(150)).await;",
    "flush_buffer(&store, &mut buffer);\n                        tokio::task::yield_now().await;\n                        tokio::time::sleep(Duration::from_millis(500)).await;"
)

with open("src/axon-core/src/graph_writer.rs", "w") as f:
    f.write(content)
