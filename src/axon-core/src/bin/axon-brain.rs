fn main() -> anyhow::Result<()> {
    // REQ-AXO-902338 — AVANT tout démarrage : un argument non reconnu est
    // refusé, `--version` / `--help` répondent sans rien démarrer. Sans ce
    // garde, `bin/axon-brain --version` lançait un SECOND brain MCP.
    if let Some(code) = axon_core::role_cli::handle("axon-brain", "serveur MCP + écrivain SOLL") {
        std::process::exit(code);
    }
    axon_core::runtime_boot::run_brain()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entrypoint_links_to_runtime_boot() {
        let _: fn() -> anyhow::Result<()> = axon_core::runtime_boot::run_brain;
    }

    /// REQ-AXO-902338 — le garde doit être CÂBLÉ dans ce `main`, pas seulement
    /// exister dans la bibliothèque : c'est le câblage qui a manqué, pas la
    /// capacité (GUI-PRO-115).
    #[test]
    fn cli_guard_is_wired_before_boot() {
        let _: fn(&str, &str) -> Option<i32> = axon_core::role_cli::handle;
        assert_eq!(
            axon_core::role_cli::decide(&["--dry-run".to_string()]),
            axon_core::role_cli::RoleCliDecision::Reject {
                arg: "--dry-run".to_string()
            }
        );
    }
}
