use core::fmt::Write;
use core::str::FromStr;
use std::vec;

use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use serde_json::{from_str as from_json_str, json, Value};
use starknet_core::types::{BlockId, BlockTag, Felt, FunctionCall};
use starknet_core::utils::get_selector_from_name;
use starknet_crypto::{poseidon_hash_many, verify};
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider, Url};

/// A field in a StarkNet type with a name and type.
#[derive(Debug, Deserialize, Serialize)]
struct FieldType {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
}

/// Definitions of StarkNet types.
///
/// This struct holds definitions of the StarknetDomain and Challenge types.
///
#[derive(Debug)]
struct Types {
    stark_net_domain: Vec<FieldType>,
    challenge: Vec<FieldType>,
}

/// Verify an Argent wallet signature on chain.
pub async fn verify_argent_signature(
    message_hash: &str,
    signature: Vec<String>,
    wallet_address: &str,
    message: &str,
    rpc_node_url: &str,
    chain_id: &str,
) -> EyreResult<bool> {
    // Convert inputs from strings to StarkNet-compatible types
    let wallet_address = Felt::from_str(wallet_address)?;
    let message_hash = Felt::from_str(message_hash)?;

    // Set up a JSON-RPC client to interact with the StarkNet blockchain
    let provider = JsonRpcClient::new(HttpTransport::new(Url::parse(rpc_node_url)?));

    // Parse the signature strings into Felt types
    let parsed_signature: Vec<Felt> = signature
        .iter()
        .map(|s| Felt::from_str(s))
        .collect::<Result<Vec<Felt>, _>>()?;

    // Formatted entry point selector (isValidSignature string) to needed format for StarkNet RPC call
    let entry_point_selector: Felt =
        Felt::from_str("0x213dfe25e2ca309c4d615a09cfc95fdb2fc7dc73fbcad12c450fe93b1f2ff9e")?;

    // Prepare a function call to verify the signature on-chain
    let function_call = FunctionCall {
        contract_address: wallet_address,
        entry_point_selector,
        calldata: {
            let mut data = vec![message_hash, Felt::from(parsed_signature.len())];
            data.extend_from_slice(&parsed_signature);
            data
        },
    };

    // Execute the function call on the latest block
    let result = provider
        .call(&function_call, BlockId::Tag(BlockTag::Latest))
        .await;

    match result {
        // If the signature is valid, verify the hash
        Ok(_) => {
            let verify = verify_signature_hash(message_hash, wallet_address, message, chain_id);
            if verify.is_ok() {
                return Ok(true);
            }
            bail!("Invalid message hash");
        }
        Err(err) => {
            bail!("Error verifying signature: {:?}", err);
        }
    }
}

/// Verify a MetaMask Snap wallet signature off chain.
pub fn verify_metamask_signature(
    message_hash: &str,
    signature: &[String],
    signing_key: &str,
    message: &str,
    wallet_address: &str,
    chain_id: &str,
) -> EyreResult<bool> {
    // Convert inputs to Felt types
    let signing_key = Felt::from_str(signing_key)?;
    let message_hash = Felt::from_str(message_hash)?;
    let wallet_address = Felt::from_str(wallet_address)?;

    // Verify the signature using the StarkNet crypto library
    let result = verify(
        &signing_key,
        &message_hash,
        &Felt::from_str(
            signature
                .first()
                .ok_or_else(|| eyre!("Invalid signature length"))?,
        )?,
        &Felt::from_str(
            signature
                .get(1)
                .ok_or_else(|| eyre!("Invalid signature length"))?,
        )?,
    );
    match result {
        // If the signature is valid, verify the hash
        Ok(true) => {
            let verify = verify_signature_hash(message_hash, wallet_address, message, chain_id);
            if verify.is_ok() {
                return Ok(true);
            }
            bail!("Invalid message hash");
        }
        Ok(false) => {
            bail!("Invalid signature");
        }
        Err(err) => {
            bail!("Error verifying signature: {:?}", err);
        }
    }
}

/// Verify the integrity of a message hash.
///
/// This function verifies the integrity of a message hash by hashing the
/// message and comparing it to the provided hash.
///
fn verify_signature_hash(
    message_hash: Felt,
    wallet_address: Felt,
    message: &str,
    chain_id: &str,
) -> EyreResult<()> {
    let types = Types {
        stark_net_domain: vec![
            FieldType {
                name: "name".to_owned(),
                field_type: "shortstring".to_owned(),
            },
            FieldType {
                name: "chainId".to_owned(),
                field_type: "felt".to_owned(),
            },
            FieldType {
                name: "version".to_owned(),
                field_type: "shortstring".to_owned(),
            },
            FieldType {
                name: "revision".to_owned(),
                field_type: "shortstring".to_owned(),
            },
        ],
        challenge: vec![
            FieldType {
                name: "nodeSignature".to_owned(),
                field_type: "string".to_owned(),
            },
            FieldType {
                name: "publicKey".to_owned(),
                field_type: "string".to_owned(),
            },
        ],
    };

    // Parse the JSON message into a structured format
    let challenge: Value = from_json_str(message)?;

    // Calculate the prefix for the message to be verified
    let message_prefix: Felt = Felt::from_str(&format!(
        "0x{}",
        "StarkNet Message"
            .chars()
            .fold(String::new(), |mut acc, c| {
                write!(acc, "{:x}", c as u32).expect("Unable to write");
                acc
            })
    ))?;

    // Encode the StarkNet domain data and calculate its hash
    let sn_domain_types = format!(
        "\"StarknetDomain\"({})",
        types
            .stark_net_domain
            .iter()
            .map(|field| format!("\"{}\":\"{}\"", field.name, field.field_type))
            .collect::<Vec<String>>()
            .join(",")
    );
    let domain_felt: Felt = get_selector_from_name(&sn_domain_types).expect("wrong type");

    let domain_data = json!({
        "name": "ServerChallenge",
        "chainId": chain_id,
        "version": "1",
        "revision": "1"
    });

    let mut encoded_domain = encode_data(&types, "StarknetDomain", &domain_data)?;
    encoded_domain.insert(0, domain_felt);
    let encoded_domain_hash = poseidon_hash_many(&encoded_domain);

    // Encode the challenge data and calculate its hash
    let challenge_types = format!(
        "\"Challenge\"({})",
        types
            .challenge
            .iter()
            .map(|field| format!("\"{}\":\"{}\"", field.name, field.field_type))
            .collect::<Vec<String>>()
            .join(",")
    );
    let challenge_felt: Felt =
        get_selector_from_name(challenge_types.as_str()).expect("wrong type");
    let mut encoded_challenge = encode_data(&types, "Challenge", &challenge)?;
    encoded_challenge.insert(0, challenge_felt);
    let encoded_challenge_hash = poseidon_hash_many(&encoded_challenge);

    // Combine the prefix, domain hash, wallet address, and challenge hash to form the full message hash
    let message = vec![
        message_prefix,
        encoded_domain_hash,
        wallet_address,
        encoded_challenge_hash,
    ];
    let server_message_hash = poseidon_hash_many(&message);
    // Compare the calculated message hash with the provided one to verify integrity
    if server_message_hash == message_hash {
        Ok(())
    } else {
        bail!("Signature is invalid");
    }
}

/// Encode a value based on its type into a StarkNet-compatible format.
fn encode_value(field_type: &str, value: &str) -> EyreResult<String> {
    match field_type {
        "felt" => {
            if value.chars().all(char::is_numeric) {
                // Convert numeric strings to actual numbers
                Ok(format!("0x{}", u64::from_str(value)?))
            } else {
                Ok(value.to_owned())
            }
        }
        "string" => {
            // Split the string into chunks of up to 31 characters
            let mut elements = Vec::new();
            let mut pending_word = String::new();
            let mut pending_word_len = 0;

            for (i, chunk) in value.as_bytes().chunks(31).enumerate() {
                let chunk_string = chunk.iter().fold(String::new(), |mut acc, &c| {
                    write!(acc, "{c:02x}").expect("Unable to write");
                    acc
                });

                if i < value.len().saturating_div(31) {
                    elements.push(format!("0x{chunk_string}")); // Prefix with "0x"
                } else {
                    pending_word = format!("0x{chunk_string}"); // Prefix with "0x"
                    pending_word_len = chunk.len();
                }
            }
            // Prepare the final array of elements including the length and pending word details
            let mut encoded_elements: Vec<Felt> = Vec::new();
            encoded_elements.push(Felt::from(elements.len() as u64)); // Add length as number

            // Convert each string chunk to Felt and push
            for element in elements {
                encoded_elements
                    .push(Felt::from_str(&element).expect("Failed to convert element to Felt"));
            }
            // Add pending word as a Felt
            encoded_elements.push(
                Felt::from_str(&pending_word).expect("Failed to convert pending_word to Felt"),
            );
            // Add pending word length as number
            encoded_elements.push(Felt::from(pending_word_len as u64));
            // Poseidon hash
            let hash = poseidon_hash_many(&encoded_elements);
            Ok(hash.to_string())
        }
        "shortstring" => {
            // Check if the value is a numeric string and handle it like "felt"
            if value.chars().all(char::is_numeric) {
                // Attempt to convert the string to a u64, returning an error if it fails
                let num_value = u64::from_str(value)
                    .map_err(|_| eyre!("Failed to parse numeric string into u64"))?;
                Ok(format!("0x{num_value:x}"))
            } else {
                // Otherwise, convert each character to its ASCII value in hexadecimal
                let hex_string: String = value.chars().fold(String::new(), |mut acc, c| {
                    write!(acc, "{:02x}", c as u8).expect("Unable to write");
                    acc
                });
                Ok(format!("0x{hex_string}"))
            }
        }
        _ => Err(eyre!("Unsupported field type")),
    }
}

/// Encode data fields into a vector of Felt values based on their types.
fn encode_data(types: &Types, type_name: &str, data: &Value) -> EyreResult<Vec<Felt>> {
    let target_type = match type_name {
        "StarknetDomain" => &types.stark_net_domain,
        "Challenge" => &types.challenge,
        _ => bail!("Type not found"),
    };

    let mut values = vec![];
    for field in target_type {
        let field_value = data
            .get(&field.name)
            .ok_or_else(|| eyre!("Field not found"))?
            .as_str()
            .ok_or_else(|| eyre!("Invalid field value"))?;
        let encoded_value = encode_value(&field.field_type, field_value)?;
        values.push(Felt::from_str(&encoded_value)?);
    }

    Ok(values)
}
