//! The JNI adapter of Rivium's embedded host. An application's JNI library is one line,
//!
//! ```ignore
//! rivium_jni::export!(class = "com/example/svc/RiviumBridge", app = my_svc::App);
//! ```
//!
//! which generates `JNI_OnLoad`. When the JVM loads the library, it registers five static
//! native methods on the bridge class; the library itself keeps `#![forbid(unsafe_code)]`.
//!
//! | Java declaration | JNI signature | Returns |
//! | --- | --- | --- |
//! | `static native int nativeStart(String[] args)` | `([Ljava/lang/String;)I` | the FFI code of [`Host::start`] |
//! | `static native int nativeStop(long timeoutMillis)` | `(J)I` | the FFI code of [`Host::stop`] |
//! | `static native int nativeStatus()` | `()I` | idle 0, starting 1, running 2, stopping 3, restarting 4, failed 5 |
//! | `static native String nativeLastError()` | `()Ljava/lang/String;` | `"<code name>: <message>"`, or an empty string |
//! | `static native String nativeVersion()` | `()Ljava/lang/String;` | `"<name> <version>"` |
//!
//! A class that declares them otherwise fails to load: `System.loadLibrary` throws
//! `UnsatisfiedLinkError`, which names the method the JVM did not find. `nativeStart` waits until the services run or fail: call it on a
//! background thread, never on Android's main thread. `nativeStop` may run on the main thread,
//! as in `onDestroy`, with a timeout of at most 2,000 ms. A null argument array counts as no
//! arguments, a null argument as an empty one, and a negative timeout as zero.
//!
//! A panic in a native method is caught and logged; the method returns −99, or an empty
//! string. That takes `panic = "unwind"`, the default: with `"abort"` a panic would end the
//! Java application, so this crate does not build.
//!
//! [`Host::start`]: rivium::embedded::Host::start
//! [`Host::stop`]: rivium::embedded::Host::stop

#[cfg(panic = "abort")]
compile_error!(
    "rivium-jni needs panic = \"unwind\": with \"abort\", a panic would end the Java application"
);

use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};
use std::sync::OnceLock;
use std::time::Duration;

use jni::objects::{JClass, JObjectArray, JString};
use jni::refs::Reference;
use jni::strings::JNIString;
use jni::sys::{JNI_ERR, JNI_VERSION_1_6, jint, jlong};
use jni::{
    AttachConfig, AttachmentExceptionPolicy, Env, EnvUnowned, JavaVM, NativeMethod, Outcome,
    jni_str,
};
use rivium::embedded::Host;
use rivium::{App, Code};

/// Generates the `JNI_OnLoad` of an application's JNI library: it registers the native
/// methods of the crate documentation on `class`, the bridge class in JNI form
/// (`com/example/svc/RiviumBridge`), for the embedded host of `app`. A library has one
/// `JNI_OnLoad`, so it has no other.
///
/// ```ignore
/// #![forbid(unsafe_code)]
///
/// rivium_jni::export!(class = "com/example/svc/RiviumBridge", app = my_svc::App);
/// ```
///
/// `JNI_OnLoad` is an `unsafe` function, for the JVM to call with a pointer to itself; Rust
/// code cannot call it without `unsafe`:
///
/// ```compile_fail,E0133
/// # struct Svc;
/// # impl rivium::App for Svc {
/// #     const NAME: &'static str = "svc";
/// #     const VERSION: &'static str = "0.1.0";
/// #     type Config = ();
/// #     fn services(_: &(), _: &rivium::AppContext) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
/// #         Ok(Vec::new())
/// #     }
/// # }
/// rivium_jni::export!(class = "com/example/svc/RiviumBridge", app = Svc);
///
/// fn main() {
///     JNI_OnLoad(std::ptr::dangling_mut(), std::ptr::null_mut());
/// }
/// ```
#[macro_export]
macro_rules! export {
    (class = $class:literal, app = $app:ty $(,)?) => {
        /// Registers the native methods of the bridge class when the JVM loads this library.
        ///
        /// # Safety
        ///
        /// Only the JVM calls it, with a pointer to itself.
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn JNI_OnLoad(
            vm: *mut $crate::__private::JavaVM,
            _reserved: *mut ::core::ffi::c_void,
        ) -> $crate::__private::jint {
            // SAFETY: only the JVM calls this function, with its pointer to itself.
            unsafe { $crate::__private::on_load::<$app>(vm, $class) }
        }
    };
}

/// What [`export!`] expands to; not public API.
#[doc(hidden)]
pub mod __private {
    pub use jni::sys::{JavaVM, jint};

    /// The body of `JNI_OnLoad`, for [`export!`](crate::export) only.
    ///
    /// # Safety
    ///
    /// `vm` is the pointer the JVM passes to `JNI_OnLoad`.
    ///
    /// ```compile_fail,E0133
    /// # struct Svc;
    /// # impl rivium::App for Svc {
    /// #     const NAME: &'static str = "svc";
    /// #     const VERSION: &'static str = "0.1.0";
    /// #     type Config = ();
    /// #     fn services(_: &(), _: &rivium::AppContext) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
    /// #         Ok(Vec::new())
    /// #     }
    /// # }
    /// rivium_jni::__private::on_load::<Svc>(std::ptr::dangling_mut(), "x/RiviumBridge");
    /// ```
    #[expect(unsafe_code, reason = "the JVM's pointer to itself")]
    pub unsafe fn on_load<A: rivium::App>(vm: *mut JavaVM, class: &str) -> jint {
        // SAFETY: the caller passes the JVM's pointer to itself.
        unsafe { super::on_load::<A>(vm, class) }
    }
}

/// The embedded host of the library's application, and its `"<name> <version>"`.
struct Bridge {
    host: Host,
    version: String,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// The FFI code of a caught panic.
const PANICKED: jint = -99;

fn bridge() -> &'static Bridge {
    BRIDGE.get().expect("JNI_OnLoad has run")
}

/// # Safety
///
/// `vm` is the pointer the JVM passes to `JNI_OnLoad`.
#[expect(unsafe_code, reason = "the JVM's pointer to itself")]
unsafe fn on_load<A: App>(vm: *mut jni::sys::JavaVM, class: &str) -> jint {
    BRIDGE.get_or_init(|| Bridge {
        host: Host::new::<A>(),
        version: format!("{} {}", A::NAME, A::VERSION),
    });
    let registered = panic::catch_unwind(|| {
        // SAFETY: the caller passes the JVM's pointer to itself.
        let vm = unsafe { JavaVM::from_raw(vm) };
        // An exception still pending when JNI_OnLoad returns is what loading the library throws.
        let pending = || AttachConfig::new().exceptions_policy(AttachmentExceptionPolicy::Ignore);
        vm.attach_current_thread_with_config(pending, None, |env| {
            let registered = register(env, class);
            if let Err(error) = &registered {
                let why =
                    format!("{class} does not declare the native methods of rivium-jni: {error}");
                let link_error = jni_str!("java/lang/UnsatisfiedLinkError");
                let _ = env.throw_new(link_error, JNIString::new(why));
            }
            registered
        })
    });
    match registered {
        Ok(Ok(())) => JNI_VERSION_1_6,
        _ => JNI_ERR,
    }
}

#[expect(unsafe_code, reason = "registering native methods")]
fn register(env: &mut Env<'_>, class: &str) -> jni::errors::Result<()> {
    // SAFETY: each pointer is a function with the ABI of a static native method of its
    // signature: the environment, the class, then the arguments and the return type.
    let methods = unsafe {
        [
            NativeMethod::from_raw_parts(
                jni_str!("nativeStart"),
                jni_str!("([Ljava/lang/String;)I"),
                native_start as *mut c_void,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("nativeStop"),
                jni_str!("(J)I"),
                native_stop as *mut c_void,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("nativeStatus"),
                jni_str!("()I"),
                native_status as *mut c_void,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("nativeLastError"),
                jni_str!("()Ljava/lang/String;"),
                native_last_error as *mut c_void,
            ),
            NativeMethod::from_raw_parts(
                jni_str!("nativeVersion"),
                jni_str!("()Ljava/lang/String;"),
                native_version as *mut c_void,
            ),
        ]
    };
    // SAFETY: every method is static, so the second parameter of each function is the class.
    unsafe { env.register_native_methods(JNIString::new(class), &methods) }
}

extern "system" fn native_start<'l>(
    mut env: EnvUnowned<'l>,
    _: JClass<'l>,
    args: JObjectArray<'l, JString<'l>>,
) -> jint {
    int(&mut env, "nativeStart", |env| {
        let args = strings(env, &args)?;
        Ok(ffi(bridge().host.start(&args)))
    })
}

extern "system" fn native_stop<'l>(mut env: EnvUnowned<'l>, _: JClass<'l>, timeout: jlong) -> jint {
    int(&mut env, "nativeStop", |_| {
        let timeout = Duration::from_millis(u64::try_from(timeout).unwrap_or(0));
        Ok(ffi(bridge().host.stop(timeout)))
    })
}

extern "system" fn native_status<'l>(mut env: EnvUnowned<'l>, _: JClass<'l>) -> jint {
    int(&mut env, "nativeStatus", |_| {
        Ok(bridge().host.status().ffi())
    })
}

extern "system" fn native_last_error<'l>(mut env: EnvUnowned<'l>, _: JClass<'l>) -> JString<'l> {
    string(&mut env, "nativeLastError", || {
        bridge().host.last_error().unwrap_or_default()
    })
}

extern "system" fn native_version<'l>(mut env: EnvUnowned<'l>, _: JClass<'l>) -> JString<'l> {
    string(&mut env, "nativeVersion", || bridge().version.clone())
}

/// The FFI value of a code the host returns; every code `start` and `stop` return has one.
fn ffi(code: Code) -> jint {
    code.ffi()
        .expect("the embedded host returns codes with FFI values")
}

/// Runs the body of a native method that returns an int. A JNI error, which only reading the
/// arguments can raise, returns the usage code; a panic is caught, the panic hook logs it, and
/// it returns −99.
fn int<'l>(
    env: &mut EnvUnowned<'l>,
    method: &str,
    body: impl FnOnce(&mut Env<'l>) -> jni::errors::Result<jint>,
) -> jint {
    let outcome = env.with_env(|env| {
        fault(method);
        body(env)
    });
    match outcome.into_outcome() {
        Outcome::Ok(value) => value,
        Outcome::Err(_) => ffi(Code::Usage),
        Outcome::Panic(_) => PANICKED,
    }
}

/// Runs the body of a native method that returns a string. A panic is caught, the panic hook
/// logs it, and it returns an empty string; null when not even that can be made.
fn string<'l>(
    env: &mut EnvUnowned<'l>,
    method: &str,
    body: impl FnOnce() -> String,
) -> JString<'l> {
    let outcome = env.with_env(|env| {
        let text = panic::catch_unwind(AssertUnwindSafe(|| {
            fault(method);
            body()
        }));
        env.new_string(text.unwrap_or_default())
    });
    match outcome.into_outcome() {
        Outcome::Ok(text) => text,
        Outcome::Err(_) | Outcome::Panic(_) => JString::default(),
    }
}

/// The arguments of `nativeStart`: a null array counts as none, a null element as an empty one.
fn strings<'l>(
    env: &mut Env<'l>,
    array: &JObjectArray<'l, JString<'l>>,
) -> jni::errors::Result<Vec<String>> {
    if array.is_null() {
        return Ok(Vec::new());
    }
    let len = array.len(env)?;
    (0..len)
        .map(|index| {
            let arg = array.get_element(env, index)?;
            match arg.is_null() {
                true => Ok(String::new()),
                false => arg.try_to_string(env),
            }
        })
        .collect()
}

/// Test builds (`--cfg rivium_jni_fault`) panic in the native methods that the environment
/// variable `RIVIUM_JNI_PANIC` names, separated by commas: the desktop JVM contract checks that
/// such a panic returns −99 or an empty string and the JVM goes on. Other builds have no such
/// code.
#[cfg(rivium_jni_fault)]
fn fault(method: &str) {
    let names = std::env::var("RIVIUM_JNI_PANIC").unwrap_or_default();
    if names.split(',').any(|name| name == method) {
        panic!("{method}: a panic injected by a test build");
    }
}

#[cfg(not(rivium_jni_fault))]
fn fault(_: &str) {}
