//! The objects the agent serves: scalars of the system group, and the device's below the
//! enterprise number 32473, which RFC 5612 reserves for documentation.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering::Relaxed};
use std::time::Instant;

use crate::snmp::{GET, GET_NEXT, Message, Pdu, RESPONSE, V2C, Value};

/// sysDescr.0: the program and its version.
pub const SYS_DESCR: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 1, 0];
/// sysObjectID.0: [`PRODUCT`].
pub const SYS_OBJECT_ID: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 2, 0];
/// sysUpTime.0: since the agent started.
pub const SYS_UP_TIME: &[u32] = &[1, 3, 6, 1, 2, 1, 1, 3, 0];
/// The product: below the documentation enterprise number.
pub const PRODUCT: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1];
/// The device's serial number, which the host passes.
pub const SERIAL: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1, 1, 0];
/// The device's last reading, an INTEGER.
pub const READING: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1, 2, 0];
/// How many polls of the device have been answered, a Counter32.
pub const POLLS: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1, 3, 0];
/// How many polls of the device have failed, a Counter32.
pub const ERRORS: &[u32] = &[1, 3, 6, 1, 4, 1, 32473, 1, 4, 0];

/// Every object, in the order of their names.
const OBJECTS: [&[u32]; 7] = [
    SYS_DESCR,
    SYS_OBJECT_ID,
    SYS_UP_TIME,
    SERIAL,
    READING,
    POLLS,
    ERRORS,
];

/// What polling the device has found, shared by the poller and the agent.
#[derive(Clone, Debug, Default)]
pub struct Readings(Arc<(AtomicI64, AtomicU32, AtomicU32)>);

impl Readings {
    pub(crate) fn record(&self, reading: i64) {
        self.0.0.store(reading, Relaxed);
        self.0.1.fetch_add(1, Relaxed);
    }

    pub(crate) fn fail(&self) {
        self.0.2.fetch_add(1, Relaxed);
    }
}

/// The objects of one agent.
pub(crate) struct Mib {
    pub(crate) description: String,
    pub(crate) serial: String,
    pub(crate) readings: Readings,
    pub(crate) started: Instant,
}

impl Mib {
    /// The answer to a GET or GETNEXT request in `community`; none to other requests, which
    /// SNMPv2c agents drop.
    pub(crate) fn answer(&self, request: &Message, community: &[u8]) -> Option<Message> {
        let kind = request.pdu.kind;
        if request.version != V2C
            || request.community != community
            || ![GET, GET_NEXT].contains(&kind)
        {
            return None;
        }
        let bindings = (request.pdu.bindings.iter())
            .map(|(name, _)| match kind {
                GET => (name.clone(), self.get(name).unwrap_or(Value::NoSuchObject)),
                _ => match OBJECTS.iter().find(|object| **object > name.as_slice()) {
                    Some(next) => (next.to_vec(), self.get(next).unwrap_or(Value::NoSuchObject)),
                    None => (name.clone(), Value::EndOfMibView),
                },
            })
            .collect();
        let pdu = Pdu {
            kind: RESPONSE,
            error_status: 0,
            error_index: 0,
            bindings,
            ..request.pdu.clone()
        };
        Some(Message {
            pdu,
            ..request.clone()
        })
    }

    fn get(&self, name: &[u32]) -> Option<Value> {
        let readings = &self.readings.0;
        Some(match name {
            SYS_DESCR => Value::OctetString(self.description.clone().into_bytes()),
            SYS_OBJECT_ID => Value::ObjectId(PRODUCT.to_vec()),
            SYS_UP_TIME => {
                let ticks = self.started.elapsed().as_millis() / 10;
                Value::TimeTicks(u32::try_from(ticks).unwrap_or(u32::MAX))
            }
            SERIAL => Value::OctetString(self.serial.clone().into_bytes()),
            READING => Value::Integer(readings.0.load(Relaxed)),
            POLLS => Value::Counter32(readings.1.load(Relaxed)),
            ERRORS => Value::Counter32(readings.2.load(Relaxed)),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mib() -> Mib {
        let readings = Readings::default();
        readings.record(215);
        Mib {
            description: "snmp-lite 1.0".to_string(),
            serial: "SN-1".to_string(),
            readings,
            started: Instant::now(),
        }
    }

    fn request(kind: u8, names: &[&[u32]]) -> Message {
        let bindings = names
            .iter()
            .map(|name| (name.to_vec(), Value::Null))
            .collect();
        Message {
            version: V2C,
            community: b"public".to_vec(),
            pdu: Pdu {
                kind,
                request_id: 7,
                error_status: 0,
                error_index: 0,
                bindings,
            },
        }
    }

    #[test]
    fn objects_are_in_the_order_of_their_names() {
        assert!(OBJECTS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn get_answers_each_name_and_get_next_walks_to_the_end() {
        let mib = mib();
        let answer = mib.answer(
            &request(GET, &[SERIAL, READING, &[1, 3, 6, 1, 9]]),
            b"public",
        );
        let answer = answer.unwrap().pdu;
        assert_eq!((answer.kind, answer.request_id), (RESPONSE, 7));
        let values: Vec<_> = answer
            .bindings
            .into_iter()
            .map(|(_, value)| value)
            .collect();
        let serial = Value::OctetString(b"SN-1".to_vec());
        assert_eq!(values, [serial, Value::Integer(215), Value::NoSuchObject]);

        let mut name = vec![1, 3, 6, 1];
        let mut walked = Vec::new();
        loop {
            let answer = mib.answer(&request(GET_NEXT, &[&name]), b"public").unwrap();
            let (next, value) = answer.pdu.bindings[0].clone();
            if value == Value::EndOfMibView {
                assert_eq!(next, ERRORS);
                break;
            }
            walked.push(next.clone());
            name = next;
        }
        assert_eq!(walked, OBJECTS.map(<[u32]>::to_vec));
    }

    #[test]
    fn other_versions_communities_and_requests_get_no_answer() {
        let mib = mib();
        let mut v1 = request(GET, &[SERIAL]);
        v1.version = 0;
        assert!(mib.answer(&v1, b"public").is_none());
        assert!(mib.answer(&request(GET, &[SERIAL]), b"private").is_none());
        assert!(mib.answer(&request(0xa3, &[SERIAL]), b"public").is_none());
    }
}
