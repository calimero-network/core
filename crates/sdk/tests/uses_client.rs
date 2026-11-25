//! The Rust `uses` client `calimero-app-abi` generates (its golden kitchen-sink client) compiles
//! against this SDK, and its described types implement `AbiType`, so an app may take or answer
//! them in its own methods.

#![cfg(not(target_arch = "wasm32"))]

include!("../../app-abi/tests/golden/kitchen-sink.uses.rs");

fn shape<T: calimero_sdk::AbiType + ?Sized>() -> &'static str {
    T::DEF.shape
}

#[test]
fn generated_types_carry_their_shapes() {
    // The target's plain numbers stay plain, described as such.
    let stats = shape::<uses::sink::Stats>();
    for field in [
        r#"["views","u64_number"]"#,
        r#"["delta","i64_number"]"#,
        r#"["pixels","byte_list"]"#,
        r#"["peak","option<u64_number>",{"optional":true}]"#,
    ] {
        assert!(stats.contains(field), "{field}: {stats}");
    }
    assert!(shape::<uses::sink::Item>().contains(r#"["id","u64"]"#));
    assert!(shape::<uses::sink::Page<()>>().contains(r#""params":["T"]"#));
}
