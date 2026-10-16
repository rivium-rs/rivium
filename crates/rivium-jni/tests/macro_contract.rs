#![allow(unsafe_code)]

use rivium_host::{App, Context};

struct Fixture;

impl App for Fixture {
    const NAME: &'static str = "jni-contract";
    const VERSION: &'static str = "0.2.0";
    type Config = ();

    fn services(_: &(), _: &Context<()>) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
        panic!("a null VM must be rejected before application construction")
    }
}

mod bridge {
    use rivium_jni as renamed;

    renamed::export!(class = "example/Contract", app = super::Fixture);
}

#[test]
fn nested_macro_entry_rejects_null_jvm_before_application_work() {
    // SAFETY: null is an explicitly handled invalid loader argument; it is never dereferenced.
    let result = unsafe { bridge::JNI_OnLoad(std::ptr::null_mut(), std::ptr::null_mut()) };
    assert_eq!(result, -1);
}
