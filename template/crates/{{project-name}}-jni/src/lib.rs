//! The service library inside an Android app: its JNI library is this one line. The app
//! declares the native methods on `{{bridge_package}}.RiviumBridge` (jvm-test/RiviumBridge.java
//! shows them), loads `lib{{crate_name}}_jni.so`, and calls `nativeStart` on a background thread
//! with `--root` set to its files directory. rivium-jni holds the unsafe code: this crate
//! forbids it.
#![forbid(unsafe_code)]

rivium_jni::export!(
    class = "{{ bridge_package | replace: ".", "/" }}/RiviumBridge",
    app = {{crate_name}}::{{project-name | pascal_case}}
);
