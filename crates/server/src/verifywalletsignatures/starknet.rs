use std::str::FromStr;
use std::vec;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starknet_core::types::{BlockId, BlockTag, Felt, FunctionCall};
use starknet_core::utils::get_selector_from_name;
use starknet_crypto::{poseidon_hash_many, verify};
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider, Url};

// Structure representing a field in a StarkNet type with a name and type
#[derive(Serialize, Deserialize, Debug)]
struct FieldType {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
}

// Structure holding definitions of StarkNet types: StarknetDomain and Challenge
#[derive(Debug)]
struct Types {
    stark_net_domain: Vec<FieldType>,
    challenge: Vec<FieldType>,
}

// Asynchronous function to verify an Argent wallet signature on chain
pub async fn verify_argent_signature(
    message_hash: &str,
    signature: Vec<String>,
    wallet_address: &str,
    message: &str,
    rpc_node_url: &str,
    chain_id: &str,
) -> eyre::Result<bool> {
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
            eyre::bail!("Invalid message hash");
        }
        Err(err) => {
            eyre::bail!("Error verifying signature: {:?}", err);
        }
    }
}

// Function to verify a MetaMask Snap wallet signature off chain
pub fn verify_metamask_signature(
    message_hash: &str,
    signature: Vec<String>,
    signing_key: &str,
    message: &str,
    wallet_address: &str,
    chain_id: &str,
) -> eyre::Result<bool> {
    // Convert inputs to Felt types
    let signing_key = Felt::from_str(&signing_key)?;
    let message_hash = Felt::from_str(&message_hash)?;
    let wallet_address = Felt::from_str(&wallet_address)?;

    // Verify the signature using the StarkNet crypto library
    let result = verify(
        &signing_key,
        &message_hash,
        &Felt::from_str(&signature[0])?,
        &Felt::from_str(&signature[1])?,
    );
    match result {
        // If the signature is valid, verify the hash
        Ok(true) => {
            let verify = verify_signature_hash(message_hash, wallet_address, message, chain_id);
            if verify.is_ok() {
                return Ok(true);
            }
            eyre::bail!("Invalid message hash");
        }
        Ok(false) => {
            eyre::bail!("Invalid signature");
        }
        Err(err) => {
            eyre::bail!("Error verifying signature: {:?}", err);
        }
    }
}

// Function to verify the integrity of a message hash by hashing the message and comparing it to the provided hash
fn verify_signature_hash(
    message_hash: Felt,
    wallet_address: Felt,
    message: &str,
    chain_id: &str,
) -> eyre::Result<()> {
    let types = Types {
        stark_net_domain: vec![
            FieldType {
                name: "name".to_string(),
                field_type: "shortstring".to_string(),
            },
            FieldType {
                name: "chainId".to_string(),
                field_type: "felt".to_string(),
            },
            FieldType {
                name: "version".to_string(),
                field_type: "shortstring".to_string(),
            },
            FieldType {
                name: "revision".to_string(),
                field_type: "shortstring".to_string(),
            },
        ],
        challenge: vec![
            FieldType {
                name: "nodeSignature".to_string(),
                field_type: "string".to_string(),
            },
            FieldType {
                name: "publicKey".to_string(),
                field_type: "string".to_string(),
            },
        ],
    };

    // Parse the JSON message into a structured format
    let challenge: Value = serde_json::from_str(message)?;

    // Calculate the prefix for the message to be verified
    let message_prefix: Felt = Felt::from_str(&format!(
        "0x{}",
        "StarkNet Message"
            .chars()
            .map(|c| format!("{:x}", c as u32))
            .collect::<String>()
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
    let domain_felt: Felt =
        get_selector_from_name(&sn_domain_types.to_string()).expect("wrong type");

    let domain_data = serde_json::json!({
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
        eyre::bail!("Signature is invalid");
    }
}

// Function to encode a value based on its type into a StarkNet-compatible format
fn encode_value(field_type: &str, value: &str) -> eyre::Result<String> {
    match field_type {
        "felt" => {
            if value.chars().all(char::is_numeric) {
                // Convert numeric strings to actual numbers
                Ok(format!("0x{}", u64::from_str(value)?.to_string()))
            } else {
                Ok(value.to_string())
            }
        }
        "string" => {
            // Split the string into chunks of up to 31 characters
            let mut elements = Vec::new();
            let mut pending_word = String::new();
            let mut pending_word_len = 0;

            for (i, chunk) in value.as_bytes().chunks(31).enumerate() {
                let chunk_string = chunk
                    .iter()
                    .map(|&c| format!("{:02x}", c))
                    .collect::<String>();

                if i < value.len() / 31 {
                    elements.push(format!("0x{}", chunk_string)); // Prefix with "0x"
                } else {
                    pending_word = format!("0x{}", chunk_string); // Prefix with "0x"
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
                    .map_err(|_| eyre::eyre!("Failed to parse numeric string into u64"))?;
                Ok(format!("0x{:x}", num_value))
            } else {
                // Otherwise, convert each character to its ASCII value in hexadecimal
                let hex_string: String =
                    value.chars().map(|c| format!("{:02x}", c as u8)).collect();
                Ok(format!("0x{}", hex_string))
            }
        }
        _ => Err(eyre::eyre!("Unsupported field type")),
    }
}

// Function to encode data fields into a vector of Felt values based on their types
fn encode_data(
    types: &Types,
    type_name: &str,
    data: &serde_json::Value,
) -> eyre::Result<Vec<Felt>> {
    let target_type = match type_name {
        "StarknetDomain" => &types.stark_net_domain,
        "Challenge" => &types.challenge,
        _ => panic!("Type not found"),
    };

    let mut values = vec![];
    for field in target_type {
        let field_value = data
            .get(&field.name)
            .ok_or_else(|| eyre::eyre!("Field not found"))?
            .as_str()
            .ok_or_else(|| eyre::eyre!("Invalid field value"))?;
        let encoded_value = encode_value(&field.field_type, field_value)?;
        values.push(Felt::from_str(&encoded_value)?);
    }

    Ok(values)
}
