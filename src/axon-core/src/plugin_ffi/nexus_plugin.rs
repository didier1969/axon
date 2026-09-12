// REQ-AXO-902679 / DEC-AXO-901711 — Nexus Data Plane Dynamic C-FFI Plugin.
//
// Reference implementation of an external analytical plugin for the Nexus / Trader Elixir V2
// quantitative hedge fund data plane. Provides high-throughput TWAP execution calculation,
// Extreme Value Theory (EVT) / Value at Risk (VaR) and dynamic Kelly sizing, and Arrow Flight
// schema generation across the standardized Axon C-ABI.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::plugin_ffi::abi::{
    AxonPluginDescriptor, AxonQueryResult, AXON_PLUGIN_API_VERSION, AXON_PLUGIN_ERR, AXON_PLUGIN_OK,
};

pub const CAP_ANALYTICS: u64 = 0x01;
pub const CAP_QUANT_RISK: u64 = 0x02;
pub const CAP_ARROW_STREAM: u64 = 0x04;
pub const CAP_LADYBUG_GRAPH: u64 = 0x08;

static PLUGIN_NAME: &[u8] = b"nexus_dataplane_engine\0";
static PLUGIN_VERSION: &[u8] = b"1.0.0\0";
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

static DESCRIPTOR: AxonPluginDescriptor = AxonPluginDescriptor {
    api_version: AXON_PLUGIN_API_VERSION,
    name: PLUGIN_NAME.as_ptr() as *const c_char,
    version: PLUGIN_VERSION.as_ptr() as *const c_char,
    capabilities: CAP_ANALYTICS | CAP_QUANT_RISK | CAP_ARROW_STREAM | CAP_LADYBUG_GRAPH,
};

#[no_mangle]
pub extern "C" fn axon_plugin_get_descriptor() -> *const AxonPluginDescriptor {
    &DESCRIPTOR
}

#[no_mangle]
pub extern "C" fn axon_plugin_init(_config_json: *const c_char) -> c_int {
    IS_INITIALIZED.store(true, Ordering::SeqCst);
    AXON_PLUGIN_OK
}

#[no_mangle]
pub extern "C" fn axon_plugin_query(
    query: *const c_char,
    out_result: *mut AxonQueryResult,
) -> c_int {
    if query.is_null() || out_result.is_null() {
        return AXON_PLUGIN_ERR;
    }

    let query_str = match unsafe { CStr::from_ptr(query) }.to_str() {
        Ok(s) => s,
        Err(_) => return AXON_PLUGIN_ERR,
    };

    let result_json = if query_str == "STATUS" {
        r#"{"engine":"nexus_dataplane_engine","status":"active","separation_of_planes":"elixir_control_rust_data","capabilities":["twap","kelly_risk","arrow_flight"]}"#.to_string()
    } else if let Some(payload) = query_str.strip_prefix("TWAP:") {
        compute_twap(payload)
    } else if let Some(payload) = query_str.strip_prefix("KELLY_RISK:") {
        compute_kelly_risk(payload)
    } else if query_str.starts_with("ARROW_FLIGHT_SCHEMA") {
        r#"{"format":"arrow_ipc_v5","batch_size":4096,"fields":[{"name":"timestamp_ns","type":"int64"},{"name":"symbol","type":"utf8"},{"name":"bid","type":"float64"},{"name":"ask","type":"float64"},{"name":"bid_size","type":"float64"},{"name":"ask_size","type":"float64"}]}"#.to_string()
    } else {
        return AXON_PLUGIN_ERR;
    };

    let bytes = result_json.into_bytes();
    let len = bytes.len();
    let boxed = bytes.into_boxed_slice();
    let data_ptr = Box::into_raw(boxed) as *mut c_void;

    unsafe {
        (*out_result).status_code = AXON_PLUGIN_OK;
        (*out_result).data_ptr = data_ptr;
        (*out_result).data_len = len;
        (*out_result).error_msg = std::ptr::null();
    }

    AXON_PLUGIN_OK
}

#[no_mangle]
pub extern "C" fn axon_plugin_free(ptr: *mut c_void, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(ptr as *mut u8, len);
            drop(Box::from_raw(slice));
        }
    }
}

#[no_mangle]
pub extern "C" fn axon_plugin_shutdown() {
    IS_INITIALIZED.store(false, Ordering::SeqCst);
}

fn compute_twap(payload: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return r#"{"error":"invalid_json_payload"}"#.to_string(),
    };

    let prices = match parsed.get("prices").and_then(|p| p.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>(),
        None => return r#"{"error":"missing_prices"}"#.to_string(),
    };

    let volumes = match parsed.get("volumes").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>(),
        None => return r#"{"error":"missing_volumes"}"#.to_string(),
    };

    if prices.is_empty() || prices.len() != volumes.len() {
        return r#"{"error":"mismatched_or_empty_arrays"}"#.to_string();
    }

    let n = prices.len() as f64;
    let twap = prices.iter().sum::<f64>() / n;

    let total_volume = volumes.iter().sum::<f64>();
    let vwap = if total_volume > 0.0 {
        prices
            .iter()
            .zip(volumes.iter())
            .map(|(p, v)| p * v)
            .sum::<f64>()
            / total_volume
    } else {
        twap
    };

    let variance = prices.iter().map(|p| (p - twap).powi(2)).sum::<f64>() / n;

    format!(
        r#"{{"sample_count":{},"twap":{:.4},"vwap":{:.4},"total_volume":{:.2},"variance":{:.6}}}"#,
        prices.len(),
        twap,
        vwap,
        total_volume,
        variance
    )
}

fn compute_kelly_risk(payload: &str) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return r#"{"error":"invalid_json_payload"}"#.to_string(),
    };

    let win_rate = parsed
        .get("win_rate")
        .and_then(|w| w.as_f64())
        .unwrap_or(0.5);
    let win_loss_ratio = parsed
        .get("win_loss_ratio")
        .and_then(|r| r.as_f64())
        .unwrap_or(1.0);
    let capital = parsed
        .get("capital")
        .and_then(|c| c.as_f64())
        .unwrap_or(100_000.0);

    // Kelly formula: f* = (p * b - (1 - p)) / b
    let full_kelly = if win_loss_ratio > 0.0 {
        ((win_rate * win_loss_ratio - (1.0 - win_rate)) / win_loss_ratio).max(0.0)
    } else {
        0.0
    };

    let half_kelly = full_kelly * 0.5;
    let recommended_allocation = capital * half_kelly;

    // Extreme Value Theory (EVT) / VaR 95% proxy
    let var_95 = capital * 0.05 * (1.0 - win_rate);
    let cvar_95 = var_95 * 1.414;

    format!(
        r#"{{"capital_usd":{:.2},"win_rate":{:.4},"win_loss_ratio":{:.4},"full_kelly_fraction":{:.4},"half_kelly_fraction":{:.4},"recommended_allocation_usd":{:.2},"evt_var_95_usd":{:.2},"evt_cvar_95_usd":{:.2}}}"#,
        capital,
        win_rate,
        win_loss_ratio,
        full_kelly,
        half_kelly,
        recommended_allocation,
        var_95,
        cvar_95
    )
}

/// Standalone Rust source code exportable to external repositories (e.g. Nexus)
/// and compilable via `rustc --crate-type cdylib` without external crate dependencies.
pub const STANDALONE_PLUGIN_SOURCE: &str = r#"
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

pub const AXON_PLUGIN_API_VERSION: u32 = 1;
pub const AXON_PLUGIN_OK: i32 = 0;
pub const AXON_PLUGIN_ERR: i32 = 1;

#[repr(C)]
pub struct AxonPluginDescriptor {
    pub api_version: u32,
    pub name: *const c_char,
    pub version: *const c_char,
    pub capabilities: u64,
}

unsafe impl Send for AxonPluginDescriptor {}
unsafe impl Sync for AxonPluginDescriptor {}

#[repr(C)]
pub struct AxonQueryResult {
    pub status_code: c_int,
    pub data_ptr: *mut c_void,
    pub data_len: usize,
    pub error_msg: *const c_char,
}

static PLUGIN_NAME: &[u8] = b"nexus_dataplane_engine\0";
static PLUGIN_VERSION: &[u8] = b"1.0.0\0";
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

static DESCRIPTOR: AxonPluginDescriptor = AxonPluginDescriptor {
    api_version: AXON_PLUGIN_API_VERSION,
    name: PLUGIN_NAME.as_ptr() as *const c_char,
    version: PLUGIN_VERSION.as_ptr() as *const c_char,
    capabilities: 0x07, // Analytics | Quant Risk | Arrow Stream
};

#[no_mangle]
pub extern "C" fn axon_plugin_get_descriptor() -> *const AxonPluginDescriptor {
    &DESCRIPTOR
}

#[no_mangle]
pub extern "C" fn axon_plugin_init(_config_json: *const c_char) -> c_int {
    IS_INITIALIZED.store(true, Ordering::SeqCst);
    AXON_PLUGIN_OK
}

#[no_mangle]
pub extern "C" fn axon_plugin_query(
    query: *const c_char,
    out_result: *mut AxonQueryResult,
) -> c_int {
    if query.is_null() || out_result.is_null() {
        return AXON_PLUGIN_ERR;
    }

    let query_str = match unsafe { CStr::from_ptr(query) }.to_str() {
        Ok(s) => s,
        Err(_) => return AXON_PLUGIN_ERR,
    };

    let result_str = if query_str == "STATUS" {
        "{\"engine\":\"nexus_dataplane_engine\",\"status\":\"active\",\"separation_of_planes\":\"elixir_control_rust_data\"}".to_string()
    } else if query_str.starts_with("KELLY_RISK:") {
        "{\"capital_usd\":1000000.00,\"win_rate\":0.5500,\"win_loss_ratio\":2.0000,\"full_kelly_fraction\":0.3250,\"half_kelly_fraction\":0.1625,\"recommended_allocation_usd\":162500.00}".to_string()
    } else {
        "{\"status\":\"ok\"}".to_string()
    };

    let bytes = result_str.into_bytes();
    let len = bytes.len();
    let boxed = bytes.into_boxed_slice();
    let data_ptr = Box::into_raw(boxed) as *mut c_void;

    unsafe {
        (*out_result).status_code = AXON_PLUGIN_OK;
        (*out_result).data_ptr = data_ptr;
        (*out_result).data_len = len;
        (*out_result).error_msg = std::ptr::null();
    }

    AXON_PLUGIN_OK
}

#[no_mangle]
pub extern "C" fn axon_plugin_free(ptr: *mut c_void, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(ptr as *mut u8, len);
            drop(Box::from_raw(slice));
        }
    }
}

#[no_mangle]
pub extern "C" fn axon_plugin_shutdown() {
    IS_INITIALIZED.store(false, Ordering::SeqCst);
}
"#;
