use calimero_primitives::identity::Did;
use libp2p::identity::Keypair;

#[derive(Clone, Debug)]
pub struct IdentityHandler {
    node_identity: Did,
}

impl IdentityHandler {
    pub fn new(node_identity: Did) -> Self {
        Self { node_identity }
    }

    pub fn get_executor_identity(&self) -> String {
        self.node_identity.id.clone()
    }

    pub fn sign_message(&mut self, message: &[u8]) -> Vec<u8> {
        let mut bytes = self.node_identity.root_keys[0]
            .clone()
            .signing_key
            .into_bytes();
        let byte_slice: &mut [u8] = &mut bytes;
        let keypair = Keypair::ed25519_from_bytes(byte_slice).unwrap();
        keypair.sign(message).unwrap()
    }
}

impl From<&Keypair> for IdentityHandler {
    fn from(keypair: &Keypair) -> Self {
        let did = Did {
            id: keypair.public().to_peer_id().to_base58(),
            root_keys: vec![],
            client_keys: vec![],
        };
        Self::new(did)
    }
}

impl From<Keypair> for IdentityHandler {
    fn from(keypair: Keypair) -> Self {
        (&keypair).into()
    }
}
