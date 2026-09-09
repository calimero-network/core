use std::collections::hash_map::Entry;

use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::Dial;
use eyre::eyre;
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::DialError as SwarmDialError;
use multiaddr::Protocol;
use tokio::sync::oneshot;
use tracing::debug;

use crate::NetworkManager;

/// Outcome of starting one dial, so the handler's decision is separable from
/// the actor plumbing it answers through.
pub(crate) enum DialStart {
    /// A dial is in flight; the sender was parked in `pending_dial`.
    Started,
    /// Nothing was started and the caller should be answered immediately —
    /// either because a dial is already in flight, or because a connection to
    /// this peer already exists.
    AlreadyUnderway,
    /// The swarm refused the dial outright.
    Refused(String),
}

impl NetworkManager {
    /// Start a dial to `peer_id` at `peer_addr`, parking `sender` for whichever
    /// swarm event resolves it.
    ///
    /// Split out of the `Dial` handler so the routing-table and
    /// peer-verification behaviour can be asserted against a real manager —
    /// `Handler::handle` needs an actix `Context`, which a test cannot
    /// fabricate.
    pub(crate) fn start_dial(
        &mut self,
        peer_id: libp2p::PeerId,
        peer_addr: multiaddr::Multiaddr,
        sender: oneshot::Sender<eyre::Result<()>>,
    ) -> DialStart {
        match self.pending_dial.entry(peer_id) {
            Entry::Occupied(_) => DialStart::AlreadyUnderway,
            Entry::Vacant(entry) => {
                let opts = DialOpts::peer_id(peer_id)
                    .condition(PeerCondition::DisconnectedAndNotDialing)
                    .addresses(vec![peer_addr])
                    .build();

                match self.swarm.dial(opts) {
                    Ok(()) => {
                        let _ignored = entry.insert(sender);
                        DialStart::Started
                    }
                    Err(SwarmDialError::DialPeerConditionFalse(condition)) => {
                        debug!(
                            %peer_id,
                            ?condition,
                            "dial skipped: already connected or dialing this peer"
                        );
                        DialStart::AlreadyUnderway
                    }
                    Err(e) => DialStart::Refused(e.to_string()),
                }
            }
        }
    }
}

impl Handler<Dial> for NetworkManager {
    type Result = Response<<Dial as Message>::Result>;

    fn handle(&mut self, Dial(mut peer_addr): Dial, _ctx: &mut Context<Self>) -> Self::Result {
        let Some(Protocol::P2p(peer_id)) = peer_addr.pop() else {
            return Response::reply(Err(eyre!("No peer ID in address: {}", peer_addr)));
        };

        let (sender, receiver) = oneshot::channel();

        match self.start_dial(peer_id, peer_addr, sender) {
            DialStart::Started => {}
            // NB: this `Ok(())` means "a dial to this peer is already in
            // flight, or a connection already exists", not "the dial
            // succeeded". For the in-flight case the running dial owns the
            // only sender, so we cannot subscribe to its result here without a
            // broadcast/clone; until that is wired up, a caller hitting this
            // branch gets a spurious success even if the real dial later fails.
            DialStart::AlreadyUnderway => return Response::reply(Ok(())),
            DialStart::Refused(err) => return Response::reply(Err(eyre!(err))),
        }

        Response::fut(async move {
            // The sender lives in `pending_dial` and is normally either fired
            // by `ConnectionEstablished`/`OutgoingConnectionError` or carried
            // until then. If it is dropped without sending — e.g. the manager
            // is shutting down and tears down the swarm — `recv` errors out.
            // Surface that as a dial error rather than panicking the actor.
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err(eyre!("dial cancelled before completion")),
            }
        })
    }
}
