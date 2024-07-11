- introduction
- core components
    - `EventLoop` - Main component with the control loop (and all components)
        - `Swarm` - Main construct in libp2p
            - `Identity` - PeerId as most important entity
            - `Runtime` - tokio is our choice
            - `Transports` - tcp and udp (quicc)
            - `Behaviour`
                - Explain how each protocol fits in our use case
                    - And a bit about interesting events from libp2p core and each protocol
                - Explain our custom stream protocol
        - `Discovery`
            We combine multiple protocols in order to improve our connectivity. 
                - At the begging we dial Calimero boot nodes (which speak Calimero KAD protocol) and we start discovering peers on local network with mDNS.
                - During identify exchange (which occurs for every established connection) we record protocols which other peer supports.
                - For discovered peers with mDNS we perform direct dial (because we want to be connected to all local peers).
                - For discovered peers with rendezvous we perform dial only if peer is not already connected (this means that it most likely already discovered and dialed due to mDNS event).
            Beside network state by lip2p (more in `self.swarm.network_info()`) we also have our discovery state.
            We keep track of multiaddrs for all connected peers and peers of interest (which are never removed from state so we can reconnect to them when needed).
            To improve connectivity, as we discover relay nodes we attempt to make a relay reservation (so other peers can hole punch us). 
            To improve connectivity, as we discover rendezvous nodes we attempt to register our external addrs (relayed addrs) and we attempt to discover other peers. Additionally, we periodically perform discovery against all discovered rendezvous nodes. At the moment we use single namespace for the renzvous (check config), but we could use contexId for namespace.
    - `Client` - Client used to interact with network (oneshot pattern)
    -  `NetworkEvents` not internal libp2p, but the ones emitted to the consumer of network
- flows
    - connectivity flow
