// REQ-AXO-902679 / DEC-AXO-901711 — Standardized C-ABI Specification for Axon Dynamic Plugins.
//
// Defines stable #[repr(C)] structures, capability flags, and function pointer signatures
// allowing external analytical engines to federate dynamically into Axon without static linking.
// Canonical source extracted to standalone micro-crate `crates/axon-plugin-abi`.

pub use axon_plugin_abi::*;
