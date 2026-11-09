//! The `calimero:app` WIT world and the allow-list of imports an app may use.

/// Absolute path of the `wit/` directory holding the world and its WASI deps.
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");

/// The fully qualified world every app component targets.
pub const WORLD: &str = "calimero:app/app@0.5.1";

/// The fully qualified world of a component holding only an app's compute methods.
pub const COMPUTE_WORLD: &str = "calimero:app/compute@0.5.1";

/// Most bytes one `wasi:random` call returns; the host refuses more and the SDK asks in pieces.
pub const MAX_RANDOM_BYTES_PER_CALL: usize = 1024 * 1024;

const ALLOWED_IMPORTS: &[&str] = &[
    "calimero:app/types@0.5.1",
    "calimero:app/storage@0.5.1",
    "calimero:app/context@0.5.1",
    "calimero:app/events@0.5.1",
    "calimero:app/blobs@0.5.1",
    "calimero:app/search@0.5.1",
    "calimero:app/files@0.5.1",
    "calimero:app/xcall@0.5.1",
    "calimero:app/action@0.5.1",
    "calimero:app/runs@0.5.1",
    "calimero:app/collections@0.5.1",
    "calimero:app/rebuild@0.5.1",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.12",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:random/random@0.2.12",
    "wasi:random/insecure-seed@0.2.12",
    "wasi:cli/stdout@0.2.12",
    "wasi:cli/stderr@0.2.12",
    "wasi:cli/environment@0.2.12",
    "wasi:http/types@0.2.12",
    "wasi:http/outgoing-handler@0.2.12",
];

const COMPUTE_ALLOWED_IMPORTS: &[&str] = &[
    "calimero:app/types@0.5.1",
    "calimero:app/files@0.5.1",
    "calimero:app/job@0.5.1",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.12",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:random/random@0.2.12",
    "wasi:random/insecure-seed@0.2.12",
    "wasi:cli/stdout@0.2.12",
    "wasi:cli/stderr@0.2.12",
    "wasi:cli/environment@0.2.12",
];

const STUBBED_IMPORTS: &[&str] = &[
    "wasi:cli/stdin@0.2.12",
    "wasi:cli/exit@0.2.12",
    "wasi:cli/terminal-input@0.2.12",
    "wasi:cli/terminal-output@0.2.12",
    "wasi:cli/terminal-stdin@0.2.12",
    "wasi:cli/terminal-stdout@0.2.12",
    "wasi:cli/terminal-stderr@0.2.12",
    "wasi:filesystem/types@0.2.12",
    "wasi:filesystem/preopens@0.2.12",
    // componentize-py 0.25.1's default build imports every sockets interface (approved by
    // Xabi, 2026-11-09): accepted at install, no socket can be created or name resolved.
    "wasi:sockets/instance-network@0.2.12",
    "wasi:sockets/ip-name-lookup@0.2.12",
    "wasi:sockets/network@0.2.12",
    "wasi:sockets/tcp-create-socket@0.2.12",
    "wasi:sockets/tcp@0.2.12",
    "wasi:sockets/udp-create-socket@0.2.12",
    "wasi:sockets/udp@0.2.12",
];

/// Interfaces the world imports; the host implements all of them.
#[must_use]
pub fn allowed_imports() -> &'static [&'static str] {
    ALLOWED_IMPORTS
}

/// Interfaces the compute world imports; a compute component may import these and the stubs.
#[must_use]
pub fn compute_allowed_imports() -> &'static [&'static str] {
    COMPUTE_ALLOWED_IMPORTS
}

/// Interfaces accepted at install but stubbed by the host (D44, plus `wasi:sockets`); not
/// part of the world.
/// Any import in neither list is refused.
#[must_use]
pub fn stubbed_imports() -> &'static [&'static str] {
    STUBBED_IMPORTS
}
