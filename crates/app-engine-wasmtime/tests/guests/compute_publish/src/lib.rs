//! Test guest: a compute component that also imports `runs`, so it can publish; the compute
//! world has no such import, so a node refuses it at prepare.

wit_bindgen::generate!({
    inline: r#"
        package test:compute-publish;
        world publisher {
            include calimero:app/compute@0.5.1;
            import calimero:app/runs@0.5.1;
        }
    "#,
    path: "../../../../app-wit/wit",
    generate_all,
});

use calimero::app::runs;

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app/compute@0.5.1","methods":{"publish":{"kind":"compute","compute":{"fuel":1,"result":"r"}}}}"#;

#[link_section = "calimero:abi"]
#[used]
static ABI: [u8; ABI_JSON.len()] = {
    let mut out = [0u8; ABI_JSON.len()];
    let mut i = 0;
    while i < ABI_JSON.len() {
        out[i] = ABI_JSON[i];
        i += 1;
    }
    out
};

struct App;

impl Guest for App {
    fn run(_method: String, args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        runs::publish(&String::from_utf8_lossy(&args))
            .map(String::into_bytes)
            .map_err(|message| AppError { code: "Publish".into(), message })
    }
}

export!(App);
