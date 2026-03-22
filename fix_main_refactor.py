with open("src/axon-core/src/main.rs", "r") as f:
    content = f.read()

# We need to replace everything from the start of the `loop { let (mut socket, _) = match listener.accept().await { ...` to the end of main.

start_idx = content.find("loop {\n        let (mut socket, _) = match listener.accept()")

if start_idx != -1:
    new_content = content[:start_idx] + """    // Start the UDS Server loop
    uds_server::start_listener(
        socket_path,
        boot_time,
        projects_root.to_string(),
        graph_store,
        batch_tx,
        parse_semaphore,
    ).await?;

    Ok(())
}
"""
    with open("src/axon-core/src/main.rs", "w") as f:
        f.write(new_content)
    print("Replaced loop successfully")
else:
    print("Could not find start idx")
