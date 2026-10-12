//! Supply-chain tests: verify that consumers can use the public API.
//!
//! These tests ensure that the crates can be consumed in different contexts
//! without causing build failures or linker errors.

#[test]
fn can_construct_app() {
    use rivium::{App, AppContext, Result, Service};
    use serde::{Deserialize, Serialize};

    #[derive(Default)]
    struct TestApp;

    #[derive(Default, Serialize, Deserialize)]
    struct Config {}

    impl App for TestApp {
        const NAME: &'static str = "test-app";
        const VERSION: &'static str = "0.0.0";
        type Config = Config;

        fn services(_config: &Self::Config, _ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
            Ok(vec![])
        }
    }

    // Just ensure it compiles and constructs.
    let _app = TestApp;
}

#[test]
fn can_use_error_types() {
    use rivium::error::{kinds, Error};

    let err = Error::explain(kinds::INTERNAL, "test error");
    assert_eq!(err.to_string(), "test error");
}
