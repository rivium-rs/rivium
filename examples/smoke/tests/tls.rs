//! rustls with the ring provider: handshake, certificate chain verification and an
//! application-data round trip.

#[test]
fn handshake_and_round_trip_with_ring() {
    let negotiated = smoke::tls_round_trip().unwrap();
    assert!(negotiated.starts_with("Some(TLSv1_3)"), "{negotiated}");
}
