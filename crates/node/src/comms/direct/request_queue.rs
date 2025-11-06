use calimero_primitives::context::ContextId;
use libp2p::PeerId;
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) struct QueueEvent {
    pub original_ctx: Option<ContextId>,
    pub original_peer: Option<PeerId>,
    pub requested_ctx: Option<ContextId>,
    pub requested_peer: Option<PeerId>,
    pub drained_count: usize,
}

#[derive(Debug)]
pub(crate) struct RequestQueue {
    rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>,
}

impl RequestQueue {
    pub(crate) fn new(rx: mpsc::Receiver<(Option<ContextId>, Option<PeerId>)>) -> Self {
        Self { rx }
    }

    pub(crate) async fn next(&mut self) -> Option<QueueEvent> {
        let (ctx, peer) = self.rx.recv().await?;

        let mut drained = 0usize;
        while let Ok((_ctx, _peer)) = self.rx.try_recv() {
            drained += 1;
        }

        let (requested_ctx, requested_peer) = if drained > 0 {
            (None, None)
        } else {
            (ctx, peer)
        };

        Some(QueueEvent {
            original_ctx: ctx,
            original_peer: peer,
            requested_ctx,
            requested_peer,
            drained_count: drained,
        })
    }
}
