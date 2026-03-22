with open("src/axon-core/src/graph_writer.rs", "r") as f:
    content = f.read()

# Since Oban will send files one by one (or in very small batches), let's reduce the rust batch size to 5 to avoid holding locks too long.
content = content.replace("if buffer.len() >= 20 {", "if buffer.len() >= 5 {")
# We can also reduce the sleep time since the lock is held for much less time.
content = content.replace("tokio::time::sleep(Duration::from_millis(1500)).await;", "tokio::time::sleep(Duration::from_millis(150)).await;")

with open("src/axon-core/src/graph_writer.rs", "w") as f:
    f.write(content)
