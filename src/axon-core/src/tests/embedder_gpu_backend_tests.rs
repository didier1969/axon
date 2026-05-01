#[cfg(test)]
mod tests {
    #[test]
    fn embedder_gpu_backend_public_surface_keeps_cuda_dispatch_strict() {
        let dispatch = crate::embedder::embedder_cuda_execution_provider_dispatch();
        let rendered = format!("{dispatch:?}");
        assert!(
            rendered.contains("error_on_failure: true"),
            "CUDA EP dispatch should stay strict across backend extraction: {rendered}"
        );
    }

    #[test]
    fn embedder_gpu_backend_public_surface_reports_missing_cuda_provider_binary() {
        let _guard = crate::tests::test_helpers::embedder_env_lock();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let ort_dir = tempdir.path().join("lib");
        std::fs::create_dir_all(&ort_dir).expect("create ort dir");
        std::fs::write(ort_dir.join("libonnxruntime.so"), b"placeholder")
            .expect("write ort dylib placeholder");

        unsafe {
            std::env::set_var(
                "ORT_DYLIB_PATH",
                ort_dir.join("libonnxruntime.so").display().to_string(),
            );
        }
        let available = crate::embedder::embedder_ort_cuda_provider_library_available();
        unsafe {
            std::env::remove_var("ORT_DYLIB_PATH");
        }

        assert!(!available);
    }
}
