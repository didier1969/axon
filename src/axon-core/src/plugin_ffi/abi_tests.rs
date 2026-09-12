use std::ffi::CString;

use crate::plugin_ffi::abi::{
    is_api_compatible, AxonPluginDescriptor, AxonQueryResult, AXON_PLUGIN_API_VERSION,
    AXON_PLUGIN_ERR, AXON_PLUGIN_ERR_UNSUPPORTED, AXON_PLUGIN_OK,
};

#[test]
fn test_plugin_abi_version_and_compatibility() {
    assert_eq!(AXON_PLUGIN_API_VERSION, 1);
    assert!(is_api_compatible(1), "Version 1 must be compatible");
    assert!(!is_api_compatible(0), "Version 0 must be rejected");
    assert!(!is_api_compatible(2), "Version 2 must be rejected");

    let name = CString::new("ladybug_engine").unwrap();
    let version = CString::new("0.1.0").unwrap();

    let desc = AxonPluginDescriptor {
        api_version: AXON_PLUGIN_API_VERSION,
        name: name.as_ptr(),
        version: version.as_ptr(),
        capabilities: 0x01 | 0x02,
    };

    assert_eq!(desc.api_version, 1);
    assert_eq!(desc.capabilities, 3);
}

#[test]
fn test_axon_query_result_layout_and_status_codes() {
    assert_eq!(AXON_PLUGIN_OK, 0);
    assert_eq!(AXON_PLUGIN_ERR, 1);
    assert_eq!(AXON_PLUGIN_ERR_UNSUPPORTED, 2);

    let err_msg = CString::new("syntax error near WHERE").unwrap();
    let res = AxonQueryResult {
        status_code: AXON_PLUGIN_ERR,
        data_ptr: std::ptr::null_mut(),
        data_len: 0,
        error_msg: err_msg.as_ptr(),
    };

    assert_eq!(res.status_code, 1);
    assert_eq!(res.data_len, 0);
    assert!(!res.error_msg.is_null());
}
