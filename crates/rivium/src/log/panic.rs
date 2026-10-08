//! The panic hook: a panic becomes an error event with target `panic`, or a line on stderr while
//! no subscriber is installed. On Android it is also written to Android's log at once.

use std::panic::PanicHookInfo;
use std::sync::Once;

use tracing::subscriber::NoSubscriber;

static HOOK: Once = Once::new();

/// Installs the panic hook, once per process: it replaces the previous hook instead of
/// wrapping it, so installing it again changes nothing.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the hosts install it (phase C-2)")
)]
pub(crate) fn install_panic_hook(name: &'static str) {
    HOOK.call_once(|| std::panic::set_hook(Box::new(move |info| report(name, info))));
}

fn report(name: &str, info: &PanicHookInfo<'_>) {
    let payload = info.payload();
    let message = (payload.downcast_ref::<&str>().copied())
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<not a string>");
    let location = info.location().map(ToString::to_string).unwrap_or_default();
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    #[cfg(target_os = "android")]
    super::logcat::write(
        name,
        tracing::Level::ERROR,
        &format!("panic in thread '{thread}' at {location}: {message}"),
    );
    // Not `tracing::enabled!`: once a scoped subscriber has existed in the process, the
    // callsite's cached interest can make it report the global subscriber as enabled when
    // none of its outputs takes the event.
    let installed = tracing::dispatcher::get_default(|current| !current.is::<NoSubscriber>());
    if installed {
        tracing::event!(
            target: "panic",
            tracing::Level::ERROR,
            panic.message = message,
            panic.location = location,
            thread.name = thread,
            "panic"
        );
    } else {
        eprintln!("{name}: panic in thread '{thread}' at {location}: {message}");
    }
}
