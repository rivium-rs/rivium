//! JNI smoke test for the CI baseline. `JNI_OnLoad` registers two static native methods of
//! `smoke.Smoke` with `RegisterNatives` (the approach rivium-jni will generate):
//!
//! | Java declaration | Behaviour |
//! | --- | --- |
//! | `static native String nativeVersion()` | returns `smoke-jni <version>` |
//! | `static native int nativePanic()` | panics inside the native method; the panic is caught and `-99` returned |
//!
//! If the Java class does not declare exactly these methods, `RegisterNatives` fails, `JNI_OnLoad`
//! returns `JNI_ERR` and `System.loadLibrary` throws `UnsatisfiedLinkError`.
#![allow(unsafe_code)] // FFI entry points; rivium-jni will keep all of this out of applications.

use std::ffi::c_void;

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_ERR, JNI_VERSION_1_6, JavaVM as RawJavaVM, jint};
use jni::{EnvUnowned, JavaVM, NativeMethod, Outcome, jni_str};

/// Called by the JVM when the library is loaded.
///
/// # Safety
///
/// `vm` must be the valid `JavaVM` pointer that the JVM passes to `JNI_OnLoad`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn JNI_OnLoad(vm: *mut RawJavaVM, _reserved: *mut c_void) -> jint {
    let registered = std::panic::catch_unwind(|| {
        // SAFETY: the JVM passes a valid JavaVM pointer to JNI_OnLoad.
        let vm = unsafe { JavaVM::from_raw(vm) };
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // SAFETY: each function pointer has the native-method ABI for the given static method
            // signature: (EnvUnowned, JClass) -> the declared return type.
            let methods = unsafe {
                [
                    NativeMethod::from_raw_parts(
                        jni_str!("nativeVersion"),
                        jni_str!("()Ljava/lang/String;"),
                        native_version as *mut c_void,
                    ),
                    NativeMethod::from_raw_parts(
                        jni_str!("nativePanic"),
                        jni_str!("()I"),
                        native_panic as *mut c_void,
                    ),
                ]
            };
            // SAFETY: both methods are static, so the second parameter of each function is a class.
            unsafe { env.register_native_methods(jni_str!("smoke/Smoke"), &methods) }
        })
    });
    match registered {
        Ok(Ok(())) => JNI_VERSION_1_6,
        _ => JNI_ERR,
    }
}

extern "system" fn native_version<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> JString<'local> {
    env.with_env(|env| env.new_string(concat!("smoke-jni ", env!("CARGO_PKG_VERSION"))))
        .resolve::<ThrowRuntimeExAndDefault>()
}

extern "system" fn native_panic<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jint {
    let outcome = env.with_env(|_env| -> jni::errors::Result<jint> {
        panic!("smoke: panic inside a native method")
    });
    match outcome.into_outcome() {
        Outcome::Ok(value) => value,
        Outcome::Err(_) => -1,
        Outcome::Panic(_) => -99,
    }
}
