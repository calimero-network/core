use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env::ext::{AccountId, ProposalId};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::types::Error;
use calimero_sdk::{app, env};
use calimero_storage::collections::{LwwRegister, Mergeable, UnorderedMap, Vector};

#[app::state(emits = Event)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AppState {
    messages: UnorderedMap<ProposalId, Vector<StoredMessage>>,
}

/// Public API representation of a message.
///
/// This struct is used for JSON serialization in the external API and data transfer.
/// It uses standard types (`String`) for ease of use but does not implement `Mergeable`,
/// so it cannot be stored directly in the application state to prevent data divergence.
#[derive(
    Clone, Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Message {
    id: String,
    author: String,
    text: String,
    created_at: String,
}

/// Internal storage representation of a message.
///
/// This struct is used for persistence within the `AppState`.
/// All fields are wrapped in `LwwRegister` (Last-Write-Wins Register) to provide
/// deterministic conflict resolution timestamps. This ensures the struct implements
/// the `Mergeable` trait required by `calimero-storage` for data consistency across nodes.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct StoredMessage {
    id: LwwRegister<String>,
    author: LwwRegister<String>,
    text: LwwRegister<String>,
    created_at: LwwRegister<String>,
}

impl Mergeable for StoredMessage {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.id.merge(&other.id);
        self.author.merge(&other.author);
        self.text.merge(&other.text);
        self.created_at.merge(&other.created_at);
        Ok(())
    }
}

impl From<Message> for StoredMessage {
    fn from(msg: Message) -> Self {
        Self {
            id: LwwRegister::new(msg.id),
            author: LwwRegister::new(msg.author),
            text: LwwRegister::new(msg.text),
            created_at: LwwRegister::new(msg.created_at),
        }
    }
}

impl From<StoredMessage> for Message {
    fn from(msg: StoredMessage) -> Self {
        Self {
            id: msg.id.into_inner(),
            author: msg.author.into_inner(),
            text: msg.text.into_inner(),
            created_at: msg.created_at.into_inner(),
        }
    }
}

#[app::event]
pub enum Event {
    ProposalCreated { id: ProposalId },
    ApprovedProposal { id: ProposalId },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]
pub struct CreateProposalRequest {
    pub action_type: String,
    pub params: serde_json::Value,
}

#[app::logic]
impl AppState {
    #[app::init]
    pub fn init() -> AppState {
        AppState {
            messages: UnorderedMap::new(),
        }
    }

    pub fn create_new_proposal(
        &mut self,
        request: CreateProposalRequest,
    ) -> Result<ProposalId, Error> {
        env::log("Starting create_new_proposal");
        env::log(&format!("Request type: {}", request.action_type));

        let proposal_id = match request.action_type.as_str() {
            "ExternalFunctionCall" => {
                env::log("Processing ExternalFunctionCall");
                let receiver_id = request.params["receiver_id"]
                    .as_str()
                    .ok_or_else(|| Error::msg("receiver_id is required"))?;
                let method_name = request.params["method_name"]
                    .as_str()
                    .ok_or_else(|| Error::msg("method_name is required"))?;
                let args = request.params["args"]
                    .as_str()
                    .ok_or_else(|| Error::msg("args is required"))?;
                let deposit = request.params["deposit"]
                    .as_str()
                    .ok_or_else(|| Error::msg("deposit is required"))?
                    .parse::<u128>()?;

                env::log(&format!(
                    "Parsed values: receiver_id={}, method_name={}, args={}, deposit={}",
                    receiver_id, method_name, args, deposit
                ));

                Self::external()
                    .propose()
                    .external_function_call(
                        receiver_id.to_string(),
                        method_name.to_string(),
                        args.to_string(),
                        deposit,
                    )
                    .send()
            }
            "Transfer" => {
                env::log("Processing Transfer");
                let receiver_id = request.params["receiver_id"]
                    .as_str()
                    .ok_or_else(|| Error::msg("receiver_id is required"))?;
                let amount = request.params["amount"]
                    .as_str()
                    .ok_or_else(|| Error::msg("amount is required"))?
                    .parse::<u128>()?;

                Self::external()
                    .propose()
                    .transfer(AccountId(receiver_id.to_string()), amount)
                    .send()
            }
            "SetContextValue" => {
                env::log("Processing SetContextValue");
                let key = request.params["key"]
                    .as_str()
                    .ok_or_else(|| Error::msg("key is required"))?
                    .as_bytes()
                    .to_vec()
                    .into_boxed_slice();
                let value = request.params["value"]
                    .as_str()
                    .ok_or_else(|| Error::msg("value is required"))?
                    .as_bytes()
                    .to_vec()
                    .into_boxed_slice();

                Self::external()
                    .propose()
                    .set_context_value(key, value)
                    .send()
            }
            "SetNumApprovals" => Self::external()
                .propose()
                .set_num_approvals(
                    request.params["num_approvals"]
                        .as_u64()
                        .ok_or(Error::msg("num_approvals is required"))? as u32,
                )
                .send(),
            "SetActiveProposalsLimit" => Self::external()
                .propose()
                .set_active_proposals_limit(
                    request.params["active_proposals_limit"]
                        .as_u64()
                        .ok_or(Error::msg("active_proposals_limit is required"))?
                        as u32,
                )
                .send(),
            "DeleteProposal" => Self::external()
                .propose()
                .delete(ProposalId(
                    hex::decode(
                        request.params["proposal_id"]
                            .as_str()
                            .ok_or_else(|| Error::msg("proposal_id is required"))?,
                    )?
                    .try_into()
                    .map_err(|_| Error::msg("Invalid proposal ID length"))?,
                ))
                .send(),
            _ => return Err(Error::msg("Invalid action type")),
        };

        env::emit(&Event::ProposalCreated { id: proposal_id });

        let old = self.messages.insert(proposal_id, Vector::new())?;
        if old.is_some() {
            return Err(Error::msg("proposal already exists"));
        }

        Ok(proposal_id)
    }

    pub fn approve_proposal(&self, proposal_id: ProposalId) -> Result<(), Error> {
        // fixme: should we need to check this?
        // self.messages
        //     .get(&proposal_id)?
        //     .ok_or(Error::msg("proposal not found"))?;

        Self::external().approve(proposal_id);

        env::emit(&Event::ApprovedProposal { id: proposal_id });

        Ok(())
    }

    pub fn get_proposal_messages(&self, proposal_id: ProposalId) -> Result<Vec<Message>, Error> {
        let Some(msgs) = self.messages.get(&proposal_id)? else {
            return Ok(vec![]);
        };

        let entries = msgs.iter()?;

        Ok(entries.map(Message::from).collect())
    }

    pub fn send_proposal_messages(
        &mut self,
        proposal_id: ProposalId,
        message: Message,
    ) -> Result<(), Error> {
        let mut messages = self.messages.get(&proposal_id)?.unwrap_or_default();

        messages.push(message.into())?;

        self.messages.insert(proposal_id, messages)?;

        Ok(())
    }
}
