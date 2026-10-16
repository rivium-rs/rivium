//! Process entrypoints. Signal ownership is enabled only here.
use crate::{
    App, Code, Options,
    session::{self, Run},
};
use rivium::StopReason;
use std::{ffi::OsString, sync::Arc};
pub fn run<A: App>() -> Code {
    run_args::<A>(std::env::args_os().skip(1))
}
pub fn run_args<A: App>(args: impl IntoIterator<Item = OsString>) -> Code {
    let options = match Options::parse(A::NAME, args, true) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return Code::Usage;
        }
    };
    if let Some(code) = session::inspect::<A>(&options) {
        return code;
    }
    let run = Arc::new(Run::new());
    session::run::<A>(options, run.clone(), false);
    run.report().map_or(Code::Fault, |report| report.code)
}
fn register<T>(call: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    #[cfg(test)]
    if FAIL_REGISTRATION.with(|remaining| match remaining.get() {
        Some(left) => {
            remaining.set(left.checked_sub(1));
            left == 0
        }
        None => false,
    }) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected signal registration failure",
        ));
    }
    call()
}
pub(crate) fn signals(
    run: Arc<Run>,
) -> std::io::Result<impl std::future::Future<Output = ()> + Send> {
    #[cfg(unix)]
    let (mut interrupt, mut terminate) = {
        use tokio::signal::unix::{SignalKind, signal};
        (
            register(|| signal(SignalKind::interrupt()))?,
            register(|| signal(SignalKind::terminate()))?,
        )
    };
    #[cfg(windows)]
    let (mut interrupt, mut interrupt_break, mut close, mut logoff, mut shutdown) = {
        use tokio::signal::windows;
        (
            register(windows::ctrl_c)?,
            register(windows::ctrl_break)?,
            register(windows::ctrl_close)?,
            register(windows::ctrl_logoff)?,
            register(windows::ctrl_shutdown)?,
        )
    };
    Ok(async move {
        #[cfg(unix)]
        loop {
            let number = tokio::select! {
                _ = interrupt.recv() => 2,
                _ = terminate.recv() => 15
            };
            run.stop(StopReason::Signal(number), None);
        }
        #[cfg(windows)]
        loop {
            let name = tokio::select! {
                _ = interrupt.recv() => "CTRL_C",
                _ = interrupt_break.recv() => "CTRL_BREAK",
                _ = close.recv() => "CTRL_CLOSE",
                _ = logoff.recv() => "CTRL_LOGOFF",
                _ = shutdown.recv() => "CTRL_SHUTDOWN"
            };
            run.stop(console_event(name), None);
        }
        #[cfg(not(any(unix, windows)))]
        while tokio::signal::ctrl_c().await.is_ok() {
            run.stop(StopReason::Signal(2), None);
        }
    })
}
#[cfg(test)]
thread_local! {
    static FAIL_REGISTRATION: std::cell::Cell<Option<usize>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(any(windows, test))]
fn console_event(name: &str) -> StopReason {
    StopReason::Signal(if matches!(name, "CTRL_C" | "CTRL_BREAK") {
        2
    } else {
        15
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn signal_registration_failure_prevents_application_start() {
        struct App;
        impl crate::App for App {
            const NAME: &'static str = "signal-failure";
            const VERSION: &'static str = "test";
            type Config = ();
            fn services(
                _: &(),
                _: &crate::Context<()>,
            ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
                panic!("signal setup must fail before services");
            }
        }
        for stage in [0, 1] {
            let root = rivium_test::TempDir::new().unwrap();
            let options = crate::Options::parse(
                <App as crate::App>::NAME,
                [
                    std::ffi::OsString::from("--root"),
                    root.path().as_os_str().into(),
                ],
                false,
            )
            .unwrap();
            let run = std::sync::Arc::new(crate::session::Run::new());
            super::FAIL_REGISTRATION.with(|failure| failure.set(Some(stage)));
            crate::session::run::<App>(options, run.clone(), false);
            super::FAIL_REGISTRATION.with(|failure| failure.set(None));
            let report = run.report().unwrap();
            assert_eq!(report.code, crate::Code::OsError);
            assert!(report.cleanup.is_success());
            assert!(report.run.is_none());
            assert!(report.primary.is_some());
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn console_control_events_stand_for_sigint_or_sigterm() {
        for (event, number) in [
            ("CTRL_C", 2),
            ("CTRL_BREAK", 2),
            ("CTRL_CLOSE", 15),
            ("CTRL_LOGOFF", 15),
            ("CTRL_SHUTDOWN", 15),
        ] {
            assert_eq!(
                super::console_event(event),
                rivium::StopReason::Signal(number)
            );
        }
    }
}
