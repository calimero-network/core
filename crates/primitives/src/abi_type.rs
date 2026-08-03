//! Identity newtypes describe themselves for the ABI, replacing the name
//! matching the syn normalizer does for types outside an app's own source.

use crate::application::ApplicationId;
use crate::blobs::BlobId;
use crate::context::ContextId;
use crate::identity::PublicKey;

calimero_wasm_abi::impl_bytes32_abi!(ApplicationId, BlobId, ContextId, PublicKey);
