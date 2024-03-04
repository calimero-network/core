#[derive(Copy, Clone, Debug)]
pub enum NodeType {
    Peer,
    Coordinator,
}

impl NodeType {
    pub fn is_coordinator(&self) -> bool {
        match *self {
            NodeType::Coordinator => true,
            NodeType::Peer => false,
        }
    }
}
