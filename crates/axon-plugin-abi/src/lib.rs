// REQ-AXO-902679 / DEC-AXO-901711 — Standardized C-ABI Specification for Axon Dynamic Plugins.
//
// Defines stable #[repr(C)] structures, capability flags, and function pointer signatures
// allowing external analytical engines (e.g. Nexus, DuckDB, Vector) to federate dynamically
// into Axon without static compilation coupling.

use std::os::raw::{c_char, c_int, c_void};

pub const AXON_PLUGIN_API_VERSION: u32 = 1;

pub const AXON_PLUGIN_OK: i32 = 0;
pub const AXON_PLUGIN_ERR: i32 = 1;
pub const AXON_PLUGIN_ERR_UNSUPPORTED: i32 = 2;

/// Checks if an external plugin's compiled API version is compatible with this host.
pub fn is_api_compatible(version: u32) -> bool {
    version == AXON_PLUGIN_API_VERSION
}

/// Metadata descriptor exported by dynamic plugins.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AxonPluginDescriptor {
    pub api_version: u32,
    pub name: *const c_char,
    pub version: *const c_char,
    pub capabilities: u64,
}

unsafe impl Send for AxonPluginDescriptor {}
unsafe impl Sync for AxonPluginDescriptor {}

/// Standard output envelope returned by analytical plugin query invocations.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AxonQueryResult {
    pub status_code: c_int,
    pub data_ptr: *mut c_void,
    pub data_len: usize,
    pub error_msg: *const c_char,
}

unsafe impl Send for AxonQueryResult {}
unsafe impl Sync for AxonQueryResult {}

// Function pointer signatures that dynamic libraries must export:
pub type AxonPluginInitFn = unsafe extern "C" fn(config_json: *const c_char) -> c_int;
pub type AxonPluginGetDescriptorFn = unsafe extern "C" fn() -> *const AxonPluginDescriptor;
pub type AxonPluginQueryFn =
    unsafe extern "C" fn(query: *const c_char, out_result: *mut AxonQueryResult) -> c_int;
pub type AxonPluginFreeFn = unsafe extern "C" fn(ptr: *mut c_void, len: usize);
pub type AxonPluginShutdownFn = unsafe extern "C" fn();

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_abi_compatibility() {
        assert!(is_api_compatible(1));
        assert!(!is_api_compatible(0));
        assert!(!is_api_compatible(2));
    }

    #[test]
    fn test_descriptor_layout() {
        let name = CString::new("test_plugin").unwrap();
        let version = CString::new("1.0.0").unwrap();

        let desc = AxonPluginDescriptor {
            api_version: AXON_PLUGIN_API_VERSION,
            name: name.as_ptr(),
            version: version.as_ptr(),
            capabilities: 0x01,
        };

        assert_eq!(desc.api_version, 1);
        assert_eq!(desc.capabilities, 1);
    }

    #[test]
    fn test_query_result_layout() {
        let res = AxonQueryResult {
            status_code: AXON_PLUGIN_OK,
            data_ptr: std::ptr::null_mut(),
            data_len: 0,
            error_msg: std::ptr::null(),
        };

        assert_eq!(res.status_code, 0);
        assert_eq!(res.data_len, 0);
    }
}
