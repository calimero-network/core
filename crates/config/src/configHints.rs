// crates/config/src/configHints.rs

#[derive(Debug)]
pub struct ConfigHint {
    pub key: &'static str,
    pub description: &'static str,
}

pub const CONFIG_HINTS: &[ConfigHint] = &[
    ConfigHint {
        key: "sync.timeout_ms",
        description: "Valid values are any positive integer in milliseconds.",
    },
    ConfigHint {
        key: "sync.interval_ms",
        description: "Valid values are any positive integer in milliseconds.",
    },
    ConfigHint {
        key: "network.swarm.port",
        description: "The port for the network swarm, valid range is 1024–65535.",
    },
    ConfigHint {
        key: "network.server.listen",
        description: "A list of addresses the server will listen on (e.g., 127.0.0.1:8080).",
    },
    ConfigHint {
        key: "datastore.path",
        description: "Path to the datastore directory (e.g., /var/data).",
    },
    ConfigHint {
        key: "blobstore.path",
        description: "Path to the blobstore directory (e.g., /var/blob).",
    },
    ConfigHint {
        key: "network.bootstrap.nodes",
        description: "A list of bootstrap nodes (multiaddress strings).",
    },
];

pub fn get_hint_for_key(key: &str) -> Option<&'static str> {
    CONFIG_HINTS.iter().find(|&hint| hint.key == key).map(|hint| hint.description)
}
