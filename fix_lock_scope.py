with open("src/axon-core/src/graph_writer.rs", "r") as f:
    content = f.read()

# Make sure the lock is dropped explicitly
content = content.replace(
    "let _ = locked.execute(\"COMMIT\");\n    info!(\"Batch flush complete: {} files, {} symbols, {} relations\", count, total_symbols, total_relations);\n    \n    buffer.clear();\n}",
    "let _ = locked.execute(\"COMMIT\");\n    drop(locked);\n    info!(\"Batch flush complete: {} files, {} symbols, {} relations\", count, total_symbols, total_relations);\n    \n    buffer.clear();\n}"
)

with open("src/axon-core/src/graph_writer.rs", "w") as f:
    f.write(content)
