//! A fixed five-method Java ABI backed by the embedded host.
//!
//! Each native library exports one application. Java declarations are checked by
//! the JVM when the library loads; integer and string calls contain Rust unwinds.
#[cfg(panic = "abort")]
compile_error!("rivium-jni requires panic=unwind for FFI panic containment");

use jni::{
    AttachConfig, AttachmentExceptionPolicy, Env, JavaVM, NativeMethod, jni_str, native_method,
    objects::{JClass, JObjectArray, JString},
    refs::Reference,
    strings::JNIString,
    sys::{JNI_ERR, JNI_VERSION_1_6, jint, jlong},
};
use rivium_host::{App, Code, embedded::Host};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::OnceLock,
    time::Duration,
};

struct ApplicationBridge {
    lifecycle: Host,
    version: String,
}
static APPLICATION: OnceLock<ApplicationBridge> = OnceLock::new();
fn application() -> &'static ApplicationBridge {
    APPLICATION
        .get()
        .expect("native registration precedes method calls")
}

/// Export the JVM's loader entry point for one application.
///
/// The class is a JNI slash-separated name; it must declare the five static
/// methods described in this crate's README.
#[macro_export]
macro_rules! export {
    (class = $java_class:literal, app = $application:ty $(,)?) => {
        /// Initialize this library when requested by a live JVM.
        ///
        /// # Safety
        /// `vm` must be the JVM-supplied pointer for the loading virtual machine.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn JNI_OnLoad(
            vm: *mut $crate::__private::JavaVM,
            _reserved: *mut ::core::ffi::c_void,
        ) -> $crate::__private::jint {
            // SAFETY: this entry point is called under the JVM loader contract.
            unsafe { $crate::__private::register::<$application>(vm, $java_class) }
        }
    };
    ($($invalid:tt)*) => {
        compile_error!("expected export!(class = \"package/Class\", app = ApplicationType)");
    };
}

/// Implementation support for macro expansion in downstream crates.
#[doc(hidden)]
pub mod __private {
    pub use jni::sys::{JavaVM, jint};
    /// # Safety
    /// The non-null pointer must identify the live JVM invoking JNI_OnLoad.
    #[allow(unsafe_code)]
    pub unsafe fn register<A: rivium_host::App>(vm: *mut JavaVM, class: &str) -> jint {
        if vm.is_null() {
            return jni::sys::JNI_ERR;
        }
        // SAFETY: the caller preserves the JNI_OnLoad pointer provenance.
        unsafe { super::register_application::<A>(vm, class) }
    }
}

#[allow(unsafe_code)]
unsafe fn register_application<A: App>(raw: *mut jni::sys::JavaVM, java_class: &str) -> jint {
    let installed = catch_unwind(AssertUnwindSafe(|| {
        APPLICATION.get_or_init(|| ApplicationBridge {
            lifecycle: Host::new::<A>(),
            version: format!("{} {}", A::NAME, A::VERSION),
        });
        // SAFETY: register_application is reached only through the JVM loader entry.
        let machine = unsafe { JavaVM::from_raw(raw) };
        machine.attach_current_thread_with_config(
            || AttachConfig::new().exceptions_policy(AttachmentExceptionPolicy::Ignore),
            None,
            |env| {
                let bindings: [NativeMethod<'_>; 5] = [
                    native_method! { static fn start(args: [JString]) -> jint, name = "nativeStart" },
                    native_method! { static fn stop(timeout: jlong) -> jint, name = "nativeStop" },
                    native_method! { static fn status() -> jint, name = "nativeStatus" },
                    native_method! { static fn last_error() -> JString, name = "nativeLastError" },
                    native_method! { static fn version() -> JString, name = "nativeVersion" },
                ];
                // SAFETY: native_method! builds static JNI wrappers with checked Rust ABIs.
                let registration = unsafe {
                    env.register_native_methods(JNIString::new(java_class), &bindings)
                };
                if let Err(error) = &registration {
                    env.throw_new(
                        jni_str!("java/lang/UnsatisfiedLinkError"),
                        JNIString::new(format!("native registration failed: {error}"))
                    )?;
                }
                registration
            },
        )
    }));
    match installed {
        Ok(Ok(())) => JNI_VERSION_1_6,
        _ => JNI_ERR,
    }
}

fn integer_call(method: &str, call: impl FnOnce() -> jni::errors::Result<jint>) -> jint {
    match catch_unwind(AssertUnwindSafe(|| {
        inject(method);
        call()
    })) {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => Code::Usage.ffi(),
        Err(_) => Code::Panicked.ffi(),
    }
}
fn text_call(method: &str, call: impl FnOnce() -> String) -> String {
    catch_unwind(AssertUnwindSafe(|| {
        inject(method);
        call()
    }))
    .unwrap_or_default()
}

fn start<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
    args: JObjectArray<'local, JString<'local>>,
) -> jni::errors::Result<jint> {
    Ok(integer_call("nativeStart", || {
        let mut arguments = Vec::new();
        if !args.is_null() {
            for index in 0..args.len(env)? {
                let item = args.get_element(env, index)?;
                arguments.push(if item.is_null() {
                    String::new()
                } else {
                    item.try_to_string(env)?
                });
            }
        }
        Ok(application().lifecycle.start(&arguments).ffi())
    }))
}
fn stop<'local>(
    _: &mut Env<'local>,
    _: JClass<'local>,
    timeout: jlong,
) -> jni::errors::Result<jint> {
    Ok(integer_call("nativeStop", || {
        Ok(application()
            .lifecycle
            .stop(Duration::from_millis(timeout.max(0) as u64))
            .ffi())
    }))
}
fn status<'local>(_: &mut Env<'local>, _: JClass<'local>) -> jni::errors::Result<jint> {
    Ok(integer_call("nativeStatus", || {
        Ok(application().lifecycle.status() as jint)
    }))
}
fn last_error<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
) -> jni::errors::Result<JString<'local>> {
    let message = text_call("nativeLastError", || {
        // Detailed configuration diagnostics stay in the Rust host, not the Java ABI.
        application()
            .lifecycle
            .last_report()
            .and_then(|report| {
                if report.code == Code::Ok {
                    None
                } else {
                    Some(format!("host result: {:?}", report.code))
                }
            })
            .unwrap_or_else(|| {
                application()
                    .lifecycle
                    .last_error()
                    .map(|_| "host operation failed".to_owned())
                    .unwrap_or_default()
            })
    });
    env.new_string(message)
}
fn version<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
) -> jni::errors::Result<JString<'local>> {
    env.new_string(text_call("nativeVersion", || application().version.clone()))
}

#[cfg(rivium_jni_fault)]
fn inject(method: &str) {
    if std::env::var("RIVIUM_JNI_PANIC")
        .unwrap_or_default()
        .split(',')
        .any(|selected| selected == method)
    {
        panic!("injected native panic: {method}");
    }
}
#[cfg(not(rivium_jni_fault))]
fn inject(_: &str) {}
