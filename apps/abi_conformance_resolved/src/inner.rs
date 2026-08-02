//! A private module, so the only name the outside world can reach `Real` by is
//! the `Renamed` re-export in `lib.rs`.

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Real {
    pub tag: String,
}
