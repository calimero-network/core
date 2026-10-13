//! How this node advertises itself to a peer that has to dial it.

/// Format this node's confirmed external addresses as multiaddrs a peer can
/// dial.
///
/// The peer id is appended because a bare address does not say who answers at
/// it, and the dialer cannot resolve that for itself: the identity-to-peer map
/// is exactly what a joiner or a follower does not have yet.
///
/// Feed it `external_addrs` from `NodeClient::network_status`, never
/// `listen_addrs`: the latter includes `0.0.0.0` and loopback, which nobody
/// else can dial, while the former already carries the relay-circuit form a
/// NAT'd node is reachable on.
///
/// Split from the lookup so the shape is testable without a live swarm. One
/// copy serves both the invitation's admitter hints and `GET /identity`, so the
/// two cannot drift apart.
pub fn dialable_self_addrs(
    local_peer_id: &impl std::fmt::Display,
    external_addrs: &[impl std::fmt::Display],
) -> Vec<String> {
    external_addrs
        .iter()
        .map(|addr| format!("{addr}/p2p/{local_peer_id}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::dialable_self_addrs;

    #[test]
    fn an_address_carries_the_peer_id_it_alone_does_not() {
        // A joiner cannot turn a bare address into "who answers there" — the
        // identity-to-peer map is precisely what it has not synced yet. So the
        // peer id travels in the hint or the hint is unusable.
        let addrs = dialable_self_addrs(&"12D3KooWTEST", &["/ip4/10.0.0.1/tcp/2528"]);
        assert_eq!(
            addrs,
            vec!["/ip4/10.0.0.1/tcp/2528/p2p/12D3KooWTEST".to_owned()]
        );
    }

    #[test]
    fn a_relay_circuit_address_is_hinted_like_any_other() {
        // The NAT'd case, and the one worth pinning: a node behind NAT is
        // reachable only through its relay reservation, so dropping circuit
        // addresses would leave exactly the nodes that most need a hint
        // advertising nothing.
        let addrs = dialable_self_addrs(
            &"12D3KooWSELF",
            &["/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWRELAY/p2p-circuit"],
        );
        assert_eq!(
            addrs,
            vec!["/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWRELAY/p2p-circuit/p2p/12D3KooWSELF".to_owned()]
        );
    }

    #[test]
    fn no_external_address_yields_no_hint_rather_than_a_useless_one() {
        let addrs = dialable_self_addrs(&"12D3KooWSELF", &[] as &[String]);
        assert!(
            addrs.is_empty(),
            "a node with no confirmed external address has nothing dialable to offer"
        );
    }
}
