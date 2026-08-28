use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let flag = args.next();
    let socket = args.next();
    if flag.as_deref() != Some(std::ffi::OsStr::new("--socket"))
        || socket.is_none()
        || args.next().is_some()
    {
        anyhow::bail!("usage: axon-query-embed-worker --socket <unix-path>");
    }
    axon_core::embedder::run_query_embed_worker(&PathBuf::from(socket.unwrap()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_entrypoint_is_linked() {
        let _: fn(&std::path::Path) -> anyhow::Result<()> =
            axon_core::embedder::run_query_embed_worker;
    }
}
