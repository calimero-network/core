use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::SetPeerScore;

use crate::NetworkManager;

impl Handler<SetPeerScore> for NetworkManager {
    type Result = <SetPeerScore as Message>::Result;

    fn handle(
        &mut self,
        SetPeerScore { peer_id, score }: SetPeerScore,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        // `set_application_score` returns false when peer scoring isn't
        // active or the peer isn't in the score book yet — both benign:
        // scoring is always enabled here, and a not-yet-known peer picks
        // up its score when it next (re)connects and the node re-pushes.
        let _applied = self
            .swarm
            .behaviour_mut()
            .gossipsub
            .set_application_score(&peer_id, score);
    }
}
