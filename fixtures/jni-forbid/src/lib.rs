//! A guard, built on stable and on the MSRV: an application's JNI library that calls
//! `rivium_jni::export!` keeps `#![forbid(unsafe_code)]`, because rustc does not report the
//! unsafe code that a macro of another crate expands to: `#[unsafe(no_mangle)]`, the
//! `unsafe extern "system" fn JNI_OnLoad` and the `unsafe` block in it. If rustc starts to, this
//! fixture stops building, and applications' JNI libraries move to `deny` while the macro gains
//! an `allow`. Built with `panic = "abort"` it must not build either: rivium-jni refuses it
//! (`ci/lint`).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

struct Guarded;

impl rivium::App for Guarded {
    const NAME: &'static str = "jni-forbid";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    type Config = BTreeMap<String, String>;

    fn services(
        _: &Self::Config,
        _: &rivium::AppContext,
    ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
        Ok(Vec::new())
    }
}

rivium_jni::export!(class = "rivium/fixture/RiviumBridge", app = Guarded);
