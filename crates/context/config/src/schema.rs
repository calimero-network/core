use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
pub struct UrlSchema(#[schemars(with = "String")] pub url::Url);

#[cfg(feature = "icp")]
#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug)]
#[serde(transparent)]
pub struct PrincipalSchema(#[schemars(with = "String")] pub candid::Principal);

#[cfg(feature = "near_client")]
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
pub struct AccountIdSchema(#[schemars(with = "String")] pub near_primitives::types::AccountId);

#[cfg(feature = "near_client")]
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
pub struct PublicKeySchema(#[schemars(with = "String")] pub near_crypto::PublicKey);

#[cfg(feature = "near_client")]
#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(transparent)]
pub struct SecretKeySchema(#[schemars(with = "String")] pub near_crypto::SecretKey);

#[cfg(feature = "starknet_client")]
#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug)]
#[serde(transparent)]
pub struct FeltSchema(#[schemars(with = "String")] pub starknet::core::types::Felt);
