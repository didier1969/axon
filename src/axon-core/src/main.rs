// Copyright (c) Didier Stadelmann. All rights reserved.
// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
#![recursion_limit = "512"]

fn main() -> anyhow::Result<()> {
    // REQ-AXO-902338 — ce binaire démarre lui aussi un indexeur complet : il
    // porte donc EXACTEMENT le même risque que `axon-indexer`, et le garde va
    // avec le comportement, pas avec le nom du fichier. Corriger l'un sans
    // l'autre aurait fermé l'instance, pas la classe.
    if let Some(code) = axon_core::role_cli::handle("axon-core", "indexeur IST (pipelines A + B)") {
        std::process::exit(code);
    }
    axon_core::runtime_boot::run_indexer()
}
