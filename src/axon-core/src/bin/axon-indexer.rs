fn main() -> anyhow::Result<()> {
    // REQ-AXO-902338 — AVANT tout démarrage : un argument non reconnu est
    // refusé, `--version` / `--help` répondent sans rien démarrer. C'est
    // exactement cette invocation — `bin/axon-indexer --version` — qui a
    // démarré un second indexeur sur l'hôte live le 2026-08-15.
    // Le drapeau interne `--__gpu-lib-probe` reste passant (REQ-AXO-902027) :
    // `run_indexer` le traite juste après.
    if let Some(code) = axon_core::role_cli::handle("axon-indexer", "indexeur IST (pipelines A + B)")
    {
        std::process::exit(code);
    }
    axon_core::runtime_boot::run_indexer()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entrypoint_links_to_runtime_boot() {
        let _: fn() -> anyhow::Result<()> = axon_core::runtime_boot::run_indexer;
    }

    /// REQ-AXO-902338 — le garde doit être CÂBLÉ dans ce `main`, pas seulement
    /// exister dans la bibliothèque (GUI-PRO-115).
    #[test]
    fn cli_guard_is_wired_before_boot() {
        let _: fn(&str, &str) -> Option<i32> = axon_core::role_cli::handle;
        assert_eq!(
            axon_core::role_cli::decide(&["--version".to_string()]),
            axon_core::role_cli::RoleCliDecision::PrintVersion
        );
    }
}
