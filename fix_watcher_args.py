import os
import re

path = '/home/dstadel/projects/axon/src/axon-core/src/fs_watcher.rs'
with open(path, 'r') as f:
    content = f.read()

# 1. stage_hot_path_delta_count
content = content.replace(
    "fn stage_hot_path_delta_count(\n    store: &GraphStore,\n    watch_root: &Path,\n    path: &Path,",
    "fn stage_hot_path_delta_count(\n    store: &GraphStore,\n    watch_root: &Path,\n    project_slug: &str,\n    path: &Path,"
)

# 2. enqueue_hot_delta_with_guard missing arguments (wait, I already replaced it but it failed or I did the wrong one)
content = content.replace(
    "pub fn enqueue_hot_delta_with_guard(\n    watch_root: &Path,\n    path: &Path,",
    "pub fn enqueue_hot_delta_with_guard(\n    watch_root: &Path,\n    project_slug: &str,\n    path: &Path,"
)
content = content.replace(
    "Ok(enqueue_hot_path_delta_count(watch_root, path, priority, Some(guard), ingress)? > 0)",
    "Ok(enqueue_hot_path_delta_count(watch_root, project_slug, path, priority, Some(guard), ingress)? > 0)"
)
content = content.replace(
    "Ok(stage_hot_path_delta_count(store, watch_root, path, priority, Some(guard))? > 0)",
    "Ok(stage_hot_path_delta_count(store, watch_root, project_slug, path, priority, Some(guard))? > 0)"
)

# 3. enqueue_hot_deltas_with_guard signature
content = content.replace(
    "pub fn enqueue_hot_deltas_with_guard(\n    watch_root: &Path,\n    paths: Vec<PathBuf>,",
    "pub fn enqueue_hot_deltas_with_guard(\n    watch_root: &Path,\n    project_slug: &str,\n    paths: Vec<PathBuf>,"
)

# 4. enqueue_hot_path_delta_count missing argument from earlier? Let's fix line 53 scanner definition:
content = content.replace(
    "let scanner = Scanner::new(watch_root.to_string_lossy().as_ref(), project_slug);",
    "let scanner = Scanner::new(watch_root.to_string_lossy().as_ref(), project_slug);"
)

with open(path, 'w') as f:
    f.write(content)
