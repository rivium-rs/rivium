//! The real native bridge, registered in a JVM without building a cdylib.
#![cfg(not(target_os = "android"))]

rivium_jni::export!(
    class = "rivium/tests/RiviumBridge",
    app = test_app::subjects::jvm::JvmApp
);

#[test]
#[allow(unsafe_code)]
fn jvm_bridge_contract() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo package directory");
    tests_integration::jvm::contract(
        &std::path::PathBuf::from(manifest).join("tests/jvm"),
        |vm| {
            // SAFETY: the test JVM is live, and this is the generated application's load hook.
            unsafe { JNI_OnLoad(vm.get_raw(), std::ptr::null_mut()) }
        },
    );
}
