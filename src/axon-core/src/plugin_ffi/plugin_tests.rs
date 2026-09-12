use std::path::Path;

use crate::plugin_ffi::manager::DynamicPluginManager;

#[test]
fn test_dynamic_plugin_handles_missing_file_gracefully() {
    let mut manager = DynamicPluginManager::new();
    let res = manager.load_plugin(
        "missing_engine",
        Path::new("/tmp/non_existent_axon_plugin_xyz.so"),
        "{}",
    );

    assert!(
        res.is_err(),
        "Loading a missing library must fail gracefully"
    );
    let err_str = res.err().unwrap().to_string();
    assert!(
        err_str.contains("failed to load dynamic library") || err_str.contains("No such file"),
        "Error must describe file loading failure: {err_str}"
    );
}

#[test]
fn test_dynamic_plugin_unregistered_query() {
    let manager = DynamicPluginManager::new();
    assert_eq!(manager.loaded_plugins().len(), 0);

    let res = manager.query("unknown_plugin", "SELECT * FROM vectors");
    assert!(res.is_err());
    let err_msg = res.err().unwrap().to_string();
    assert!(err_msg.contains("not found"));
}

#[test]
fn test_in_memory_plugin_mock_registration() {
    let mut manager = DynamicPluginManager::new();

    manager.register_mock_plugin("mock_vector_engine", "1.0.0", 0x01, |query: &str| {
        if query.contains("COUNT") {
            Ok(b"42".to_vec())
        } else {
            Ok(b"{\"status\":\"ok\"}".to_vec())
        }
    });

    assert_eq!(manager.loaded_plugins().len(), 1);
    assert!(manager.has_plugin("mock_vector_engine"));

    let count_res = manager
        .query("mock_vector_engine", "SELECT COUNT(*) FROM table")
        .expect("query count succeeds");
    assert_eq!(count_res, b"42");

    let status_res = manager
        .query("mock_vector_engine", "GET_STATUS")
        .expect("query status succeeds");
    assert_eq!(status_res, b"{\"status\":\"ok\"}");

    assert!(manager.unload_plugin("mock_vector_engine"));
    assert!(!manager.has_plugin("mock_vector_engine"));
}
