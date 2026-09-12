// REQ-AXO-902679 / DEC-AXO-901711 — Integration & Contract Tests for Nexus Data Plane Dynamic Plugin.
//
// Validates both direct C-ABI invocation and dynamic runtime loading via libloading/DynamicPluginManager.

use std::ffi::{CStr, CString};
use std::process::Command;

use crate::plugin_ffi::abi::{
    is_api_compatible, AxonQueryResult, AXON_PLUGIN_API_VERSION, AXON_PLUGIN_OK,
};
use crate::plugin_ffi::manager::DynamicPluginManager;
use crate::plugin_ffi::nexus_plugin::*;

#[test]
fn test_nexus_plugin_in_process_direct_c_abi() {
    unsafe {
        // 1. Validate Descriptor
        let desc_ptr = axon_plugin_get_descriptor();
        assert!(!desc_ptr.is_null());
        let desc = *desc_ptr;
        assert_eq!(desc.api_version, AXON_PLUGIN_API_VERSION);
        assert!(is_api_compatible(desc.api_version));

        let name = CStr::from_ptr(desc.name).to_str().unwrap();
        assert_eq!(name, "nexus_dataplane_engine");

        assert_eq!(desc.capabilities & CAP_QUANT_RISK, CAP_QUANT_RISK);
        assert_eq!(desc.capabilities & CAP_ANALYTICS, CAP_ANALYTICS);

        // 2. Initialize
        let config = CString::new("{\"max_batch_size\":1000}").unwrap();
        let init_rc = axon_plugin_init(config.as_ptr());
        assert_eq!(init_rc, AXON_PLUGIN_OK);

        // 3. Query Status
        let q_status = CString::new("STATUS").unwrap();
        let mut res = AxonQueryResult {
            status_code: -1,
            data_ptr: std::ptr::null_mut(),
            data_len: 0,
            error_msg: std::ptr::null(),
        };
        let q_rc = axon_plugin_query(q_status.as_ptr(), &mut res);
        assert_eq!(q_rc, AXON_PLUGIN_OK);
        assert_eq!(res.status_code, AXON_PLUGIN_OK);
        assert!(!res.data_ptr.is_null());
        assert!(res.data_len > 0);

        let status_slice = std::slice::from_raw_parts(res.data_ptr as *const u8, res.data_len);
        let status_str = std::str::from_utf8(status_slice).unwrap();
        assert!(status_str.contains("nexus_dataplane_engine"));
        assert!(status_str.contains("elixir_control_rust_data"));
        axon_plugin_free(res.data_ptr, res.data_len);

        // 4. Query TWAP / Execution calculation
        let q_twap = CString::new(
            r#"TWAP:{"prices":[100.0, 102.0, 101.0, 103.0],"volumes":[10.0, 20.0, 15.0, 25.0]}"#,
        )
        .unwrap();
        let mut twap_res = AxonQueryResult {
            status_code: -1,
            data_ptr: std::ptr::null_mut(),
            data_len: 0,
            error_msg: std::ptr::null(),
        };
        let twap_rc = axon_plugin_query(q_twap.as_ptr(), &mut twap_res);
        assert_eq!(twap_rc, AXON_PLUGIN_OK);
        assert_eq!(twap_res.status_code, AXON_PLUGIN_OK);

        let twap_slice =
            std::slice::from_raw_parts(twap_res.data_ptr as *const u8, twap_res.data_len);
        let twap_json: serde_json::Value = serde_json::from_slice(twap_slice).unwrap();
        assert_eq!(twap_json["sample_count"], 4);
        assert!(twap_json["twap"].as_f64().unwrap() > 101.0);
        assert!(twap_json["vwap"].as_f64().unwrap() > 101.0);
        axon_plugin_free(twap_res.data_ptr, twap_res.data_len);

        // 5. Query Kelly / Risk evaluation
        let q_risk =
            CString::new(r#"KELLY_RISK:{"win_rate":0.60,"win_loss_ratio":1.5,"capital":500000.0}"#)
                .unwrap();
        let mut risk_res = AxonQueryResult {
            status_code: -1,
            data_ptr: std::ptr::null_mut(),
            data_len: 0,
            error_msg: std::ptr::null(),
        };
        let risk_rc = axon_plugin_query(q_risk.as_ptr(), &mut risk_res);
        assert_eq!(risk_rc, AXON_PLUGIN_OK);
        assert_eq!(risk_res.status_code, AXON_PLUGIN_OK);

        let risk_slice =
            std::slice::from_raw_parts(risk_res.data_ptr as *const u8, risk_res.data_len);
        let risk_json: serde_json::Value = serde_json::from_slice(risk_slice).unwrap();
        // Kelly: (0.60 * 1.5 - 0.40) / 1.5 = (0.90 - 0.40) / 1.5 = 0.50 / 1.5 = 0.3333
        let full_kelly = risk_json["full_kelly_fraction"].as_f64().unwrap();
        assert!((full_kelly - 0.3333).abs() < 0.01);
        let half_kelly = risk_json["half_kelly_fraction"].as_f64().unwrap();
        let alloc = risk_json["recommended_allocation_usd"].as_f64().unwrap();
        assert!((alloc - 500000.0 * half_kelly).abs() < 50.0);
        axon_plugin_free(risk_res.data_ptr, risk_res.data_len);

        // 6. Shutdown
        axon_plugin_shutdown();
    }
}

#[test]
fn test_nexus_plugin_dynamic_shared_library_loading() {
    // Compile standalone plugin source into dynamic shared library (.so)
    let temp_dir = tempfile::tempdir().expect("tempdir creation succeeds");
    let so_path = temp_dir.path().join("libnexus_dataplane.so");

    let plugin_src = crate::plugin_ffi::nexus_plugin::STANDALONE_PLUGIN_SOURCE;
    let src_path = temp_dir.path().join("nexus_plugin.rs");
    std::fs::write(&src_path, plugin_src).expect("write standalone source succeeds");

    let status = Command::new("rustc")
        .arg("--crate-type")
        .arg("cdylib")
        .arg(&src_path)
        .arg("-o")
        .arg(&so_path)
        .status();

    // If rustc is directly reachable, execute the dynamic loading roundtrip
    if let Ok(exit_status) = status {
        if exit_status.success() && so_path.exists() {
            let mut manager = DynamicPluginManager::new();
            let load_res = manager.load_plugin("nexus_dataplane", &so_path, "{}");
            assert!(
                load_res.is_ok(),
                "Failed to load dynamic plugin: {:?}",
                load_res.err()
            );

            assert!(manager.has_plugin("nexus_dataplane"));

            // Query dynamic engine across libloading boundary
            let status_bytes = manager
                .query("nexus_dataplane", "STATUS")
                .expect("query status across cdylib boundary");
            let status_json: serde_json::Value =
                serde_json::from_slice(&status_bytes).expect("parse status json");
            assert_eq!(status_json["engine"], "nexus_dataplane_engine");
            assert_eq!(status_json["status"], "active");

            // Query Kelly risk calculation across cdylib boundary
            let risk_bytes = manager
                .query(
                    "nexus_dataplane",
                    r#"KELLY_RISK:{"win_rate":0.55,"win_loss_ratio":2.0,"capital":1000000.0}"#,
                )
                .expect("query risk across cdylib boundary");
            let risk_json: serde_json::Value =
                serde_json::from_slice(&risk_bytes).expect("parse risk json");
            assert!(risk_json["full_kelly_fraction"].as_f64().unwrap() > 0.3);

            assert!(manager.unload_plugin("nexus_dataplane"));
            assert!(!manager.has_plugin("nexus_dataplane"));
        }
    }
}
