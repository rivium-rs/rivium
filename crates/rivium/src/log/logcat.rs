//! Android's log: events are written at once through liblog, tagged with the service name.
//! The one module with unsafe code: the call into liblog.
#![cfg_attr(target_os = "android", allow(unsafe_code))]

/// Logcat keeps entries up to about 4 KiB; longer text is written in pieces.
#[cfg_attr(
    not(target_os = "android"),
    expect(dead_code, reason = "tested on every platform")
)]
const PIECE: usize = 4_000;

/// `text` in pieces of at most `max` bytes, cut at character boundaries.
pub(super) fn pieces(mut text: &str, max: usize) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            // A character longer than `max` goes whole.
            end = text.chars().next().map_or(text.len(), char::len_utf8);
        }
        let (piece, rest) = text.split_at(end);
        text = rest;
        Some(piece)
    })
}

#[cfg(target_os = "android")]
pub(super) use android::{Logcat, write};

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::io;

    use tracing::{Level, Metadata};
    use tracing_subscriber::fmt::writer::MakeWriter;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    /// Writes `text` to Android's log at the priority of `level`.
    pub(in crate::log) fn write(tag: &str, level: Level, text: &str) {
        let Ok(tag) = CString::new(tag.replace('\0', " ")) else {
            return;
        };
        write_c(&tag, priority(level), text);
    }

    fn write_c(tag: &CStr, priority: c_int, text: &str) {
        for piece in super::pieces(text.trim_end_matches('\n'), super::PIECE) {
            let Ok(piece) = CString::new(piece.replace('\0', " ")) else {
                continue;
            };
            // SAFETY: both pointers are to NUL-terminated strings that live through the call;
            // liblog only reads them.
            unsafe { __android_log_write(priority, tag.as_ptr(), piece.as_ptr()) };
        }
    }

    /// Android's priorities: VERBOSE 2, DEBUG 3, INFO 4, WARN 5, ERROR 6.
    fn priority(level: Level) -> c_int {
        match level {
            Level::TRACE => 2,
            Level::DEBUG => 3,
            Level::INFO => 4,
            Level::WARN => 5,
            Level::ERROR => 6,
        }
    }

    /// The output layer's writer: one entry per event, at the event's priority.
    pub(in crate::log) struct Logcat {
        tag: CString,
    }

    impl Logcat {
        pub(in crate::log) fn new(name: &str) -> Logcat {
            Logcat {
                tag: CString::new(name.replace('\0', " ")).unwrap_or_default(),
            }
        }
    }

    pub(in crate::log) struct Entry<'a> {
        tag: &'a CStr,
        priority: c_int,
    }

    impl<'a> MakeWriter<'a> for Logcat {
        type Writer = Entry<'a>;

        fn make_writer(&'a self) -> Entry<'a> {
            Entry {
                tag: &self.tag,
                priority: priority(Level::INFO),
            }
        }

        fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Entry<'a> {
            Entry {
                tag: &self.tag,
                priority: priority(*meta.level()),
            }
        }
    }

    impl io::Write for Entry<'_> {
        fn write(&mut self, event: &[u8]) -> io::Result<usize> {
            write_c(self.tag, self.priority, &String::from_utf8_lossy(event));
            Ok(event.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
