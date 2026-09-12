// REQ-AXO-902679 / DEC-AXO-901711 — DynamicPluginManager via libloading.
//
// Dynamically loads, validates, queries, and unloads external C-FFI plugins
// without linking them statically into the axon-core host binary.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use libloading::{Library, Symbol};

use crate::plugin_ffi::abi::{
    is_api_compatible, AxonPluginFreeFn, AxonPluginGetDescriptorFn, AxonPluginInitFn,
    AxonPluginQueryFn, AxonPluginShutdownFn, AxonQueryResult, AXON_PLUGIN_OK,
};

trait PluginBackend: Send + Sync {
    fn query(&self, query_str: &str) -> Result<Vec<u8>>;
    fn shutdown(&self);
}

struct NativePlugin {
    _library: Library,
    query_fn: Symbol<'static, AxonPluginQueryFn>,
    free_fn: Symbol<'static, AxonPluginFreeFn>,
    shutdown_fn: Option<Symbol<'static, AxonPluginShutdownFn>>,
}

impl PluginBackend for NativePlugin {
    fn query(&self, query_str: &str) -> Result<Vec<u8>> {
        let c_query = CString::new(query_str).context("invalid query string for C-FFI")?;
        let mut raw_result = AxonQueryResult {
            status_code: 0,
            data_ptr: std::ptr::null_mut(),
            data_len: 0,
            error_msg: std::ptr::null(),
        };

        let rc = unsafe { (self.query_fn)(c_query.as_ptr(), &mut raw_result) };
        if rc != AXON_PLUGIN_OK || raw_result.status_code != AXON_PLUGIN_OK {
            let err_msg = if !raw_result.error_msg.is_null() {
                unsafe { CStr::from_ptr(raw_result.error_msg) }
                    .to_string_lossy()
                    .to_string()
            } else {
                format!("plugin query failed with rc={rc}")
            };
            bail!("Plugin query error: {err_msg}");
        }

        if raw_result.data_ptr.is_null() || raw_result.data_len == 0 {
            return Ok(Vec::new());
        }

        let slice = unsafe {
            std::slice::from_raw_parts(raw_result.data_ptr as *const u8, raw_result.data_len)
        };
        let result_bytes = slice.to_vec();

        // Release buffer memory via plugin-provided free
        unsafe {
            (self.free_fn)(raw_result.data_ptr, raw_result.data_len);
        }

        Ok(result_bytes)
    }

    fn shutdown(&self) {
        if let Some(ref shutdown) = self.shutdown_fn {
            unsafe {
                shutdown();
            }
        }
    }
}

struct MockPlugin {
    handler: Box<dyn Fn(&str) -> Result<Vec<u8>> + Send + Sync>,
}

impl PluginBackend for MockPlugin {
    fn query(&self, query_str: &str) -> Result<Vec<u8>> {
        (self.handler)(query_str)
    }

    fn shutdown(&self) {}
}

pub struct LoadedPluginInfo {
    pub name: String,
    pub version: String,
    pub capabilities: u64,
    backend: Box<dyn PluginBackend>,
}

/// Central manager for dynamically federated plugins.
pub struct DynamicPluginManager {
    plugins: HashMap<String, LoadedPluginInfo>,
}

impl DynamicPluginManager {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn loaded_plugins(&self) -> Vec<String> {
        self.plugins.keys().cloned().collect()
    }

    pub fn has_plugin(&self, name: &str) -> bool {
        self.plugins.contains_key(name)
    }

    pub fn load_plugin(&mut self, name: &str, path: &Path, config_json: &str) -> Result<()> {
        let lib = unsafe { Library::new(path) }
            .with_context(|| format!("failed to load dynamic library at {:?}", path))?;

        // Leak library to obtain 'static lifetime for symbols safely held in manager
        let lib_static: &'static Library = Box::leak(Box::new(lib));

        let get_desc: Symbol<'static, AxonPluginGetDescriptorFn> = unsafe {
            lib_static
                .get(b"axon_plugin_get_descriptor")
                .context("missing axon_plugin_get_descriptor symbol")?
        };

        let desc_ptr = unsafe { get_desc() };
        if desc_ptr.is_null() {
            bail!("axon_plugin_get_descriptor returned null pointer");
        }

        let desc = unsafe { *desc_ptr };
        if !is_api_compatible(desc.api_version) {
            bail!(
                "incompatible plugin API version {}: host requires {}",
                desc.api_version,
                crate::plugin_ffi::abi::AXON_PLUGIN_API_VERSION
            );
        }

        let plugin_version = if !desc.version.is_null() {
            unsafe { CStr::from_ptr(desc.version) }
                .to_string_lossy()
                .to_string()
        } else {
            "unknown".to_string()
        };

        let init_fn: Symbol<'static, AxonPluginInitFn> = unsafe {
            lib_static
                .get(b"axon_plugin_init")
                .context("missing axon_plugin_init symbol")?
        };

        let c_config = CString::new(config_json)?;
        let init_rc = unsafe { init_fn(c_config.as_ptr()) };
        if init_rc != AXON_PLUGIN_OK {
            bail!("axon_plugin_init returned error code {init_rc}");
        }

        let query_fn: Symbol<'static, AxonPluginQueryFn> = unsafe {
            lib_static
                .get(b"axon_plugin_query")
                .context("missing axon_plugin_query symbol")?
        };

        let free_fn: Symbol<'static, AxonPluginFreeFn> = unsafe {
            lib_static
                .get(b"axon_plugin_free")
                .context("missing axon_plugin_free symbol")?
        };

        let shutdown_fn: Option<Symbol<'static, AxonPluginShutdownFn>> =
            unsafe { lib_static.get(b"axon_plugin_shutdown").ok() };

        let backend = Box::new(NativePlugin {
            _library: unsafe { std::ptr::read(lib_static) },
            query_fn,
            free_fn,
            shutdown_fn,
        });

        self.plugins.insert(
            name.to_string(),
            LoadedPluginInfo {
                name: name.to_string(),
                version: plugin_version,
                capabilities: desc.capabilities,
                backend,
            },
        );

        Ok(())
    }

    pub fn register_mock_plugin<F>(
        &mut self,
        name: &str,
        version: &str,
        capabilities: u64,
        handler: F,
    ) where
        F: Fn(&str) -> Result<Vec<u8>> + Send + Sync + 'static,
    {
        self.plugins.insert(
            name.to_string(),
            LoadedPluginInfo {
                name: name.to_string(),
                version: version.to_string(),
                capabilities,
                backend: Box::new(MockPlugin {
                    handler: Box::new(handler),
                }),
            },
        );
    }

    pub fn query(&self, name: &str, query_str: &str) -> Result<Vec<u8>> {
        let plugin = self
            .plugins
            .get(name)
            .ok_or_else(|| anyhow!("plugin '{name}' not found in loaded plugins registry"))?;

        plugin.backend.query(query_str)
    }

    pub fn unload_plugin(&mut self, name: &str) -> bool {
        if let Some(plugin) = self.plugins.remove(name) {
            plugin.backend.shutdown();
            true
        } else {
            false
        }
    }
}

impl Default for DynamicPluginManager {
    fn default() -> Self {
        Self::new()
    }
}
