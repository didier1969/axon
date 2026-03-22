with open("src/axon-core/src/graph_writer.rs", "r") as f:
    content = f.read()

content = content.replace("tokio::time::sleep(Duration::from_millis(150)).await;", "tokio::task::yield_now().await;\n                                tokio::time::sleep(Duration::from_millis(1500)).await;")

with open("src/axon-core/src/graph_writer.rs", "w") as f:
    f.write(content)
