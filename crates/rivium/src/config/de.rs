//! Deserializers that check the form and range of a value while the configuration is read, so
//! that a bad value is reported with its key and source before anything starts. Each has a
//! serializer that writes the same form, for the default configuration:
//!
//! ```
//! use std::time::Duration;
//!
//! #[derive(serde::Serialize, serde::Deserialize)]
//! struct Poll {
//!     /// From 1 second to 1 hour, written like "30s" or "5m".
//!     #[serde(
//!         serialize_with = "rivium::config::de::serialize_duration",
//!         deserialize_with = "rivium::config::de::duration::<_, 1, 3600>"
//!     )]
//!     every: Duration,
//!     /// From 1 to 100.
//!     #[serde(deserialize_with = "rivium::config::de::integer::<_, u8, 1, 100>")]
//!     retries: u8,
//! }
//! ```
//!
//! Values from environment variables and `--set` arrive as text; these accept it as well.

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use serde::Serializer;
use serde::de::{self, Deserializer, Unexpected, Visitor};

const DURATION: &str = r#"a duration like "30s", "250ms", "5m" or "1h""#;
const BYTES: &str = r#"a size in bytes, or like "512KiB", "16MiB" or "1GiB""#;
const UNITS: [(&str, u64); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];

/// A duration from `MIN` to `MAX` seconds, written as a whole number and a unit: `ms`, `s`, `m`
/// or `h`.
///
/// # Errors
///
/// When the value is not such a duration, or is out of range.
pub fn duration<'de, D: Deserializer<'de>, const MIN: u64, const MAX: u64>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    struct Read<const MIN: u64, const MAX: u64>;
    impl<const MIN: u64, const MAX: u64> Visitor<'_> for Read<MIN, MAX> {
        type Value = Duration;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(DURATION)
        }
        fn visit_str<E: de::Error>(self, text: &str) -> Result<Duration, E> {
            let units = [("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)];
            let millis = number_with_unit(text, &units)
                .ok_or_else(|| E::invalid_value(Unexpected::Str(text), &self))?;
            let (min, max) = (MIN.saturating_mul(1_000), MAX.saturating_mul(1_000));
            match millis {
                Some(millis) if (min..=max).contains(&millis) => Ok(Duration::from_millis(millis)),
                _ => Err(E::custom(format!(
                    "must be between {} and {}, got {text:?}",
                    format_duration(Duration::from_secs(MIN)),
                    format_duration(Duration::from_secs(MAX)),
                ))),
            }
        }
    }
    deserializer.deserialize_any(Read::<MIN, MAX>)
}

/// A size from `MIN` to `MAX` bytes: a number of bytes, or a whole number with the unit `KiB`,
/// `MiB` or `GiB`.
///
/// # Errors
///
/// When the value is not such a size, or is out of range.
pub fn bytes<'de, D: Deserializer<'de>, const MIN: u64, const MAX: u64>(
    deserializer: D,
) -> Result<u64, D::Error> {
    struct Read<const MIN: u64, const MAX: u64>;
    impl<const MIN: u64, const MAX: u64> Read<MIN, MAX> {
        fn check<E: de::Error>(value: Option<u64>, shown: &dyn fmt::Display) -> Result<u64, E> {
            value
                .filter(|value| (MIN..=MAX).contains(value))
                .ok_or_else(|| {
                    let (min, max) = (format_bytes(MIN), format_bytes(MAX));
                    E::custom(format!("must be between {min} and {max}, got {shown}"))
                })
        }
    }
    impl<const MIN: u64, const MAX: u64> Visitor<'_> for Read<MIN, MAX> {
        type Value = u64;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(BYTES)
        }
        fn visit_i64<E: de::Error>(self, value: i64) -> Result<u64, E> {
            Self::check(u64::try_from(value).ok(), &value)
        }
        fn visit_u64<E: de::Error>(self, value: u64) -> Result<u64, E> {
            Self::check(Some(value), &value)
        }
        fn visit_str<E: de::Error>(self, text: &str) -> Result<u64, E> {
            let units: Vec<(&str, u64)> = UNITS.iter().copied().chain([("", 1)]).collect();
            let value = number_with_unit(text, &units)
                .ok_or_else(|| E::invalid_value(Unexpected::Str(text), &self))?;
            Self::check(value, &format_args!("{text:?}"))
        }
    }
    deserializer.deserialize_any(Read::<MIN, MAX>)
}

/// An integer from `MIN` to `MAX`, as a `T`.
///
/// # Errors
///
/// When the value is not an integer, or is out of range.
pub fn integer<'de, D: Deserializer<'de>, T: TryFrom<i64>, const MIN: i64, const MAX: i64>(
    deserializer: D,
) -> Result<T, D::Error> {
    struct Read<T, const MIN: i64, const MAX: i64>(PhantomData<T>);
    impl<T: TryFrom<i64>, const MIN: i64, const MAX: i64> Read<T, MIN, MAX> {
        fn check<E: de::Error>(value: Option<i64>, shown: &dyn fmt::Display) -> Result<T, E> {
            (value.filter(|value| (MIN..=MAX).contains(value)))
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| E::custom(format!("must be between {MIN} and {MAX}, got {shown}")))
        }
    }
    impl<T: TryFrom<i64>, const MIN: i64, const MAX: i64> Visitor<'_> for Read<T, MIN, MAX> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an integer from {MIN} to {MAX}")
        }
        fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
            Self::check(Some(value), &value)
        }
        fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
            Self::check(i64::try_from(value).ok(), &value)
        }
        fn visit_str<E: de::Error>(self, text: &str) -> Result<T, E> {
            let digits = text.strip_prefix('-').unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(E::invalid_value(Unexpected::Str(text), &self));
            }
            Self::check(text.parse().ok(), &format_args!("{text:?}"))
        }
    }
    deserializer.deserialize_any(Read::<T, MIN, MAX>(PhantomData))
}

/// Writes a duration as [`duration`] reads it, in the largest unit that is exact.
///
/// # Errors
///
/// Whatever the serializer reports.
pub fn serialize_duration<S: Serializer>(
    value: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_duration(*value))
}

/// Writes a size as [`bytes`] reads it: in the largest unit that is exact, else in bytes.
///
/// # Errors
///
/// Whatever the serializer reports.
pub fn serialize_bytes<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    match UNITS
        .iter()
        .find(|(_, size)| *value != 0 && value.is_multiple_of(*size))
    {
        Some(_) => serializer.serialize_str(&format_bytes(*value)),
        None => serializer.serialize_u64(*value),
    }
}

/// `<digits><unit>` with one of the units as (name, multiplier): `Some(value)` when the number
/// fits, `None` inside when it is too large, and `None` when the text has another form.
fn number_with_unit(text: &str, units: &[(&str, u64)]) -> Option<Option<u64>> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    let (_, size) = units
        .iter()
        .find(|(name, _)| *name == unit)
        .filter(|_| digits > 0)?;
    Some(
        number
            .parse::<u64>()
            .ok()
            .and_then(|number| number.checked_mul(*size)),
    )
}

/// A duration in the largest unit that is exact: `250ms`, `30s`, `5m`, `1h`.
pub(crate) fn format_duration(value: Duration) -> String {
    let millis = value.as_millis();
    let units = [("h", 3_600_000), ("m", 60_000), ("s", 1_000)];
    match units
        .iter()
        .find(|(_, size)| millis != 0 && millis.is_multiple_of(*size))
    {
        Some((unit, size)) => format!("{}{unit}", millis / size),
        None if millis == 0 => "0s".to_string(),
        None => format!("{millis}ms"),
    }
}

/// A size in the largest binary unit that is exact, else in bytes: `16MiB`, `1000`.
pub(crate) fn format_bytes(value: u64) -> String {
    match UNITS
        .iter()
        .find(|(_, size)| value != 0 && value.is_multiple_of(*size))
    {
        Some((unit, size)) => format!("{}{unit}", value / size),
        None => value.to_string(),
    }
}
