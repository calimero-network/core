use calimero_node::NodeMode;
use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum NodeModeArg {
    /// Standard mode - full node functionality with JSON-RPC execution
    #[default]
    Standard,
    /// Read-only mode - disables JSON-RPC execution, used for TEE observer nodes
    ReadOnly,
}

impl From<NodeModeArg> for NodeMode {
    fn from(value: NodeModeArg) -> Self {
        match value {
            NodeModeArg::Standard => NodeMode::Standard,
            NodeModeArg::ReadOnly => NodeMode::ReadOnly,
        }
    }
}
