// REQ-AXO-902679 / DEC-AXO-901711 — Dynamic C-FFI Plugin Engine for Axon.
//
// Enables zero-static-coupling dynamic federation of heavy analytical engines
// (LadybugDB, DuckDB, Vector) via standardized #[repr(C)] ABI loaded at runtime
// via libloading.

pub mod abi;
pub mod manager;
pub mod nexus_plugin;

#[cfg(test)]
mod abi_tests;
#[cfg(test)]
mod nexus_plugin_tests;
#[cfg(test)]
mod plugin_tests;
