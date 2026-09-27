//! The attest endpoint's key binding is opt-in, so existing clients keep
//! getting the same request and response shapes.

use calimero_server_primitives::admin::TeeAttestRequest;

const NONCE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn a_request_without_the_flag_does_not_bind_the_key() {
    let req: TeeAttestRequest =
        serde_json::from_str(&format!(r#"{{"nonce":"{NONCE}","applicationId":null}}"#)).unwrap();
    assert!(!req.bind_node_key);
}

#[test]
fn a_request_can_ask_for_the_key_binding() {
    let req: TeeAttestRequest = serde_json::from_str(&format!(
        r#"{{"nonce":"{NONCE}","applicationId":null,"bindNodeKey":true}}"#
    ))
    .unwrap();
    assert!(req.bind_node_key);
    assert!(
        TeeAttestRequest::new(NONCE.to_owned(), None)
            .with_node_key_binding()
            .bind_node_key
    );
}

#[test]
fn a_request_can_ask_for_the_transport_key_binding() {
    let req: TeeAttestRequest = serde_json::from_str(&format!(
        r#"{{"nonce":"{NONCE}","applicationId":null,"bindTransportKey":true}}"#
    ))
    .unwrap();
    assert!(req.bind_transport_key);
    assert!(
        !req.bind_node_key,
        "the two bindings are asked for separately"
    );
    assert!(
        TeeAttestRequest::new(NONCE.to_owned(), None)
            .with_transport_key_binding()
            .bind_transport_key
    );
}

#[test]
fn a_request_can_ask_for_the_tls_key_binding() {
    let absent: TeeAttestRequest =
        serde_json::from_str(&format!(r#"{{"nonce":"{NONCE}","applicationId":null}}"#)).unwrap();
    assert!(!absent.bind_tls_key, "off unless asked for");

    let req: TeeAttestRequest = serde_json::from_str(&format!(
        r#"{{"nonce":"{NONCE}","applicationId":null,"bindTlsKey":true}}"#
    ))
    .unwrap();
    assert!(req.bind_tls_key);
    assert!(
        !req.bind_transport_key && !req.bind_node_key,
        "each binding is asked for separately"
    );
    assert!(
        TeeAttestRequest::new(NONCE.to_owned(), None)
            .with_tls_key_binding()
            .bind_tls_key
    );
}
