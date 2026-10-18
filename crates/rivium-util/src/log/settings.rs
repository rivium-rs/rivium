use serde::{
    Deserializer, Serializer,
    de::{self, Visitor},
};
use std::fmt;

fn scaled(value: &str, units: &[(&str, u64)]) -> std::result::Result<u64, &'static str> {
    let text = value.trim();
    for (suffix, factor) in units {
        if let Some(number) = text.strip_suffix(suffix)
            && let Ok(number) = number.trim().parse::<u64>()
        {
            return number.checked_mul(*factor).ok_or("quantity overflows u64");
        }
    }
    Err("invalid quantity or unit")
}

pub(super) mod size {
    use super::*;

    pub(crate) fn serialize<S: Serializer>(
        value: &u64,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_u64(*value)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<u64, D::Error> {
        struct Bytes;

        impl Visitor<'_> for Bytes {
            type Value = u64;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unsigned bytes or an integer with B/KiB/MiB/GiB")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<u64, E> {
                Ok(value)
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<u64, E> {
                u64::try_from(value).map_err(E::custom)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<u64, E> {
                scaled(
                    value,
                    &[
                        ("GiB", 1 << 30),
                        ("MiB", 1 << 20),
                        ("KiB", 1 << 10),
                        ("B", 1),
                        ("", 1),
                    ],
                )
                .map_err(E::custom)
            }
        }
        deserializer.deserialize_any(Bytes)
    }
}

#[cfg(feature = "log-export")]
pub(super) mod duration {
    use super::*;
    use std::time::Duration;

    pub(crate) fn serialize<S: Serializer>(
        value: &Duration,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{}ms", value.as_millis()))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Duration, D::Error> {
        struct Time;

        impl Visitor<'_> for Time {
            type Value = Duration;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an integer duration ending in ms/s/m")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Duration, E> {
                scaled(value, &[("ms", 1), ("s", 1000), ("m", 60000)])
                    .map(Duration::from_millis)
                    .map_err(E::custom)
            }
        }
        deserializer.deserialize_str(Time)
    }
}
