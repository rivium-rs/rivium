//! A real application library for the Java dynamic-loading contract.
#![forbid(unsafe_code)]

rivium_jni::export!(
    class = "rivium/tests/RiviumBridge",
    app = test_app::subjects::jvm::JvmApp
);
