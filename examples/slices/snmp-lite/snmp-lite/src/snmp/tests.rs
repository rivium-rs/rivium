use super::*;

/// `snmpget -v2c -c public <agent> 1.3.6.1.2.1.1.1.0`, request id 0x12345678.
const GET_SYS_DESCR: [u8; 43] = [
    0x30, 0x29, 0x02, 0x01, 0x01, 0x04, 0x06, b'p', b'u', b'b', b'l', b'i', b'c', 0xa0, 0x1c, 0x02,
    0x04, 0x12, 0x34, 0x56, 0x78, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00, 0x30, 0x0e, 0x30, 0x0c, 0x06,
    0x08, 0x2b, 0x06, 0x01, 0x02, 0x01, 0x01, 0x01, 0x00, 0x05, 0x00,
];

fn get(bindings: Vec<(Vec<u32>, Value)>) -> Message {
    Message {
        version: V2C,
        community: b"public".to_vec(),
        pdu: Pdu {
            kind: GET,
            request_id: 0x1234_5678,
            error_status: 0,
            error_index: 0,
            bindings,
        },
    }
}

#[test]
fn a_get_request_decodes_and_encodes_byte_for_byte() {
    let message = get(vec![(vec![1, 3, 6, 1, 2, 1, 1, 1, 0], Value::Null)]);
    assert_eq!(Message::decode(&GET_SYS_DESCR), Ok(message.clone()));
    assert_eq!(message.encode(), GET_SYS_DESCR);
}

#[test]
fn integers_take_the_fewest_octets() {
    let cases: [(i64, &[u8]); 8] = [
        (0, &[0x00]),
        (127, &[0x7f]),
        (128, &[0x00, 0x80]),
        (256, &[0x01, 0x00]),
        (-1, &[0xff]),
        (-128, &[0x80]),
        (-129, &[0xff, 0x7f]),
        (i64::from(u32::MAX), &[0x00, 0xff, 0xff, 0xff, 0xff]),
    ];
    for (n, octets) in cases {
        assert_eq!(integer(n), octets, "{n}");
        assert_eq!(int(octets), Ok(n), "{n}");
    }
}

#[test]
fn every_value_type_round_trips() {
    let name = vec![1, 3, 6, 1, 4, 1, 32473, 1, 1, 0];
    let values = [
        Value::Integer(-42),
        Value::OctetString(vec![b'x'; 300]),
        Value::Null,
        Value::ObjectId(vec![1, 3, 6, 1, 4, 1, 32473, 1]),
        Value::ObjectId(vec![2, 999, u32::MAX]),
        Value::Counter32(u32::MAX),
        Value::Gauge32(7),
        Value::TimeTicks(123_456),
        Value::NoSuchObject,
        Value::NoSuchInstance,
        Value::EndOfMibView,
    ];
    let mut message = get(values.iter().map(|v| (name.clone(), v.clone())).collect());
    message.pdu.kind = RESPONSE;
    assert_eq!(Message::decode(&message.encode()), Ok(message));
}

#[test]
fn the_documentation_enterprise_number_takes_three_octets() {
    // 32473 = 1 * 128^2 + 125 * 128 + 89
    assert_eq!(
        oid(&[1, 3, 6, 1, 4, 1, 32473, 1]),
        [0x2b, 0x06, 0x01, 0x04, 0x01, 0x81, 0xfd, 0x59, 0x01]
    );
}

#[test]
fn malformed_input_is_an_error_never_a_panic() {
    for len in 0..GET_SYS_DESCR.len() {
        assert!(Message::decode(&GET_SYS_DESCR[..len]).is_err(), "{len}");
    }
    // Every value of every octet: decoding answers, whatever it answers.
    for at in 0..GET_SYS_DESCR.len() {
        for byte in 0..=u8::MAX {
            let mut bytes = GET_SYS_DESCR;
            bytes[at] = byte;
            let _ = Message::decode(&bytes);
        }
    }
    let cases: [&[u8]; 5] = [
        &[0x30, 0x80],                         // indefinite length
        &[0x30, 0x85, 0, 0, 0, 0, 1],          // a length of five octets
        &[0x30, 0x03, 0x02, 0x00, 0x00],       // an integer of no octets
        &[0x06, 0x02, 0x2b, 0x86],             // a truncated arc, outside a message
        &[0x30, 0x05, 0x02, 0x01, 0x01, 0x04], // truncated
    ];
    for bytes in cases {
        assert!(Message::decode(bytes).is_err(), "{bytes:02x?}");
    }
    assert_eq!(arcs(&[0x2b, 0x86]), Err(Malformed("a truncated arc")));
    assert_eq!(
        arcs(&[0x90, 0x80, 0x80, 0x80, 0x00]),
        Err(Malformed("an arc too large"))
    );
}
