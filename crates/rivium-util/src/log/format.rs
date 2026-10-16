use super::{Format, files::Worker};
use std::{
    fmt::{self, Write as _},
    io::Write as _,
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

pub(super) const EVENT_LIMIT: usize = 64 * 1024;

struct Buffer {
    text: String,
    overflow: bool,
}

impl Buffer {
    fn new() -> Self {
        Self {
            text: String::with_capacity(256),
            overflow: false,
        }
    }
}

impl fmt::Write for Buffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.overflow || value.len() > EVENT_LIMIT - self.text.len() {
            self.overflow = true;
            return Err(fmt::Error);
        }
        // Geometric String growth can otherwise reserve beyond the advertised limit.
        if self.text.capacity() < self.text.len() + value.len() {
            let capacity = (self.text.len() + value.len())
                .next_power_of_two()
                .min(EVENT_LIMIT);
            self.text.reserve_exact(capacity - self.text.len());
        }
        self.text.push_str(value);
        Ok(())
    }
}

struct Escaped<'a>(&'a mut Buffer);

impl fmt::Write for Escaped<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for ch in value.chars() {
            match ch {
                '"' => self.0.write_str("\\\"")?,
                '\\' => self.0.write_str("\\\\")?,
                '\n' => self.0.write_str("\\n")?,
                '\r' => self.0.write_str("\\r")?,
                '\t' => self.0.write_str("\\t")?,
                c if c < ' ' => write!(self.0, "\\u{:04x}", c as u32)?,
                c => self.0.write_char(c)?,
            }
        }
        Ok(())
    }
}

fn quoted(buffer: &mut Buffer, value: &str) {
    let _ = buffer.write_char('"');
    let _ = Escaped(buffer).write_str(value);
    let _ = buffer.write_char('"');
}

struct Fields<'a> {
    out: &'a mut Buffer,
    json: bool,
    first: bool,
}

impl Fields<'_> {
    fn name(&mut self, field: &Field) {
        if self.json {
            if !self.first {
                let _ = self.out.write_char(',');
            }
            quoted(self.out, field.name());
            let _ = self.out.write_char(':');
        } else {
            let _ = write!(self.out, " {}=", field.name());
        }
        self.first = false;
    }
}

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.name(field);
        if self.json {
            let _ = self.out.write_char('"');
            let _ = write!(Escaped(self.out), "{value:?}");
            let _ = self.out.write_char('"');
        } else {
            let _ = write!(self.out, "{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.name(field);
        if self.json {
            quoted(self.out, value);
        } else {
            let _ = self.out.write_str(value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.name(field);
        let _ = write!(self.out, "{value}");
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.name(field);
        let _ = write!(self.out, "{value}");
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.name(field);
        let _ = write!(self.out, "{value}");
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.name(field);
        if self.json && !value.is_finite() {
            let _ = self.out.write_str("null");
        } else {
            let _ = write!(self.out, "{value}");
        }
    }
}

struct SpanFields {
    values: std::collections::BTreeMap<String, String>,
    bytes: usize,
    overflow: bool,
}

impl SpanFields {
    fn new() -> Self {
        Self {
            values: std::collections::BTreeMap::new(),
            bytes: 0,
            overflow: false,
        }
    }

    fn save(&mut self, field: &Field, value: Buffer) {
        let previous = self.values.get(field.name());
        let old = previous.map_or(0, String::len);
        let key_bytes = if previous.is_none() {
            field.name().len()
        } else {
            0
        };
        if value.overflow || self.bytes - old + value.text.len() + key_bytes > EVENT_LIMIT {
            self.overflow = true;
            return;
        }
        self.bytes = self.bytes - old + value.text.len() + key_bytes;
        self.values.insert(field.name().to_owned(), value.text);
    }
}

impl Visit for SpanFields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let mut out = Buffer::new();
        let _ = write!(out, "{value:?}");
        self.save(field, out);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let mut out = Buffer::new();
        let _ = out.write_str(value);
        self.save(field, out);
    }
}

enum Output {
    File(Worker, usize),
    Console {
        format: Format,
        color: bool,
    },
    #[cfg(target_os = "android")]
    Logcat(std::ffi::CString),
}

pub(super) struct EventLayer {
    output: Output,
}

impl EventLayer {
    pub(super) fn file(worker: Worker, category: usize) -> Self {
        Self {
            output: Output::File(worker, category),
        }
    }

    pub(super) fn console(format: Format, color: bool) -> Self {
        Self {
            output: Output::Console { format, color },
        }
    }

    #[cfg(target_os = "android")]
    pub(super) fn logcat(name: &str) -> Self {
        Self {
            output: Output::Logcat(std::ffi::CString::new(name).expect("validated ASCII log name")),
        }
    }
}

impl<S> Layer<S> for EventLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if extensions.get_mut::<SpanFields>().is_none() {
                let mut fields = SpanFields::new();
                attrs.record(&mut fields);
                extensions.insert(fields);
            }
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if let Some(fields) = extensions.get_mut::<SpanFields>() {
                values.record(fields);
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let json = matches!(
            self.output,
            Output::Console {
                format: Format::Json,
                ..
            }
        );
        let color = matches!(
            self.output,
            Output::Console {
                color: true,
                format: Format::Text
            }
        );
        let mut out = Buffer::new();
        let metadata = event.metadata();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        if json {
            let _ = write!(
                out,
                "{{\"timestamp\":{}.{:03},\"level\":",
                now.as_secs(),
                now.subsec_millis()
            );
            quoted(&mut out, metadata.level().as_str());
            let _ = out.write_str(",\"target\":");
            quoted(&mut out, metadata.target());
        } else {
            if color {
                let _ = out.write_str(match *metadata.level() {
                    tracing::Level::ERROR => "\x1b[31m",
                    tracing::Level::WARN => "\x1b[33m",
                    tracing::Level::INFO => "\x1b[32m",
                    _ => "\x1b[36m",
                });
            }
            let _ = write!(
                out,
                "{}.{:03} {} {}",
                now.as_secs(),
                now.subsec_millis(),
                metadata.level(),
                metadata.target()
            );
        }
        event.record(&mut Fields {
            out: &mut out,
            json,
            first: !json,
        });
        if json {
            let _ = out.write_str(",\"spans\":[");
        }
        if let Some(scope) = ctx.event_scope(event) {
            for (i, span) in scope.from_root().enumerate() {
                if json {
                    if i > 0 {
                        let _ = out.write_char(',');
                    }
                    let _ = out.write_str("{\"name\":");
                    quoted(&mut out, span.name());
                    let _ = out.write_str(",\"fields\":");
                } else {
                    let _ = write!(out, " [{}", span.name());
                }
                if let Some(fields) = span.extensions().get::<SpanFields>() {
                    if fields.overflow {
                        out.overflow = true;
                    }
                    if json {
                        let _ = out.write_char('{');
                    }
                    for (index, (key, value)) in fields.values.iter().enumerate() {
                        if json {
                            if index != 0 {
                                let _ = out.write_char(',');
                            }
                            quoted(&mut out, key);
                            let _ = out.write_char(':');
                            quoted(&mut out, value);
                        } else {
                            let _ = write!(out, " {key}={value}");
                        }
                    }
                    if json {
                        let _ = out.write_char('}');
                    }
                } else if json {
                    let _ = out.write_str("{}");
                }
                let _ = out.write_char(if json { '}' } else { ']' });
            }
        }
        if json {
            let _ = out.write_str("]}");
        }
        if color {
            let _ = out.write_str("\x1b[0m");
        }
        let _ = out.write_char('\n');
        match &self.output {
            Output::File(worker, category) => {
                if out.overflow {
                    worker.oversized();
                } else {
                    worker.push(*category, out.text.into_bytes());
                }
            }
            Output::Console { .. } => {
                if !out.overflow {
                    let _ = std::io::stdout().lock().write_all(out.text.as_bytes());
                }
            }
            #[cfg(target_os = "android")]
            Output::Logcat(tag) => {
                if !out.overflow {
                    android_write(tag, metadata.level(), &out.text);
                }
            }
        }
    }
}

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn android_write(tag: &std::ffi::CStr, level: &tracing::Level, value: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(
            priority: std::ffi::c_int,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> std::ffi::c_int;
    }
    let priority = match *level {
        tracing::Level::ERROR => 6,
        tracing::Level::WARN => 5,
        tracing::Level::INFO => 4,
        tracing::Level::DEBUG => 3,
        tracing::Level::TRACE => 2,
    };
    let mut chunk = String::with_capacity(3800);
    let emit = |chunk: &str| {
        if let Ok(text) = std::ffi::CString::new(chunk) {
            // SAFETY: both pointers reference live NUL-terminated C strings for
            // the entire synchronous call; Android does not retain them.
            unsafe {
                __android_log_write(priority, tag.as_ptr(), text.as_ptr());
            }
        }
    };
    for ch in value.chars() {
        let ch = if ch == '\0' { '\u{fffd}' } else { ch };
        if chunk.len() + ch.len_utf8() > 3800 {
            emit(&chunk);
            chunk.clear();
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        emit(&chunk);
    }
}
