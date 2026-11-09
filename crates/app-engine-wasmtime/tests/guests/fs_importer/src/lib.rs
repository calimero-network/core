//! Test guest: binds and connects a TCP socket through std, so it imports wasi:sockets,
//! and creates a UDP socket and resolves a name through the WIT directly; every one must
//! be refused.

wit_bindgen::generate!({
    path: "../../../../app-wit/wit",
    world: "app",
    generate_all,
});

/// The socket interfaces the app world does not name (it gets them through `wasi:cli`),
/// bound directly so the guest can ask for a UDP socket and a lookup.
mod net {
    wit_bindgen::generate!({
        path: "../../../../app-wit/wit",
        inline: "package probe:net;
            world net {
                import wasi:sockets/instance-network@0.2.12;
                import wasi:sockets/udp-create-socket@0.2.12;
                import wasi:sockets/ip-name-lookup@0.2.12;
            }",
        world: "probe:net/net",
        generate_all,
    });
    pub use self::wasi::sockets;
}

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"bind":{"kind":"call"},"connect":{"kind":"call"},"udp":{"kind":"call"},"lookup":{"kind":"call"}}}"#;

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
    fn handle(_kind: InvocationKind, method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        let fail = |code: &str, message: String| AppError {
            code: code.into(),
            message,
        };
        match method.as_str() {
            "bind" => match std::net::TcpListener::bind("127.0.0.1:0") {
                Ok(_) => Ok(b"bound".to_vec()),
                Err(e) => Err(fail("Bind", e.to_string())),
            },
            "connect" => match std::net::TcpStream::connect("127.0.0.1:80") {
                Ok(_) => Ok(b"connected".to_vec()),
                Err(e) => Err(fail("Connect", e.to_string())),
            },
            "udp" => {
                use net::sockets::network::IpAddressFamily;
                match net::sockets::udp_create_socket::create_udp_socket(IpAddressFamily::Ipv4) {
                    Ok(_) => Ok(b"created".to_vec()),
                    Err(e) => Err(fail("Udp", format!("{e:?}"))),
                }
            }
            "lookup" => {
                let network = net::sockets::instance_network::instance_network();
                let stream = net::sockets::ip_name_lookup::resolve_addresses(&network, "localhost")
                    .map_err(|e| fail("Lookup", format!("{e:?}")))?;
                loop {
                    match stream.resolve_next_address() {
                        Ok(Some(_)) => return Ok(b"resolved".to_vec()),
                        Ok(None) => return Ok(b"none".to_vec()),
                        Err(net::sockets::network::ErrorCode::WouldBlock) => {
                            stream.subscribe().block()
                        }
                        Err(e) => return Err(fail("Lookup", format!("{e:?}"))),
                    }
                }
            }
            _ => Err(fail("UnknownMethod", method)),
        }
    }
}

export!(App);
