use calimero_sdk::app;

#[app::ephemeral]
pub struct Presence {
    pub cursor: u32,
    pub name: String,
}

#[test]
fn ephemeral_struct_roundtrips() {
    let p = Presence {
        cursor: 42,
        name: "x".into(),
    };
    let bytes = borsh::to_vec(&p).unwrap();
    let back: Presence = borsh::from_slice(&bytes).unwrap();
    assert_eq!(back.cursor, 42);
    assert_eq!(back.name, "x");
}
