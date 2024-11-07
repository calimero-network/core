use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env::{self};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::types::Error;
use calimero_storage::collections::UnorderedMap;
use calimero_storage::entities::Element;
use calimero_storage::AtomicUnit;

#[derive(Clone, Debug, PartialEq, PartialOrd, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct CreateProposalRequest {}

#[derive(Clone, Debug, PartialEq, PartialOrd, Deserialize)]
#[serde(crate = "calimero_sdk::serde", rename_all = "camelCase")]
pub struct GetProposalMessagesRequest {
    proposal_id: String,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Deserialize)]
#[serde(crate = "calimero_sdk::serde", rename_all = "camelCase")]
pub struct SendProposalMessageRequest {
    proposal_id: String,
    message: Message,
}

#[app::event]
pub enum Event {
    ProposalCreated(),
}

#[app::state(emits = Event)]
#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[root]
#[type_id(1)]
pub struct AppState {
    count: u32,
    #[storage]
    storage: Element,

    messages: UnorderedMap<env::ext::ProposalId, Vec<Message>>,
}

#[derive(
    Clone, Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Message {
    id: String,
    proposal_id: String,
    author: String,
    text: String,
    created_at: String,
}

#[app::logic]
impl AppState {
    #[app::init]
    pub fn init() -> AppState {
        AppState {
            count: 0,
            storage: Element::root(),
            messages: UnorderedMap::new().unwrap(),
        }
    }

    pub fn create_new_proposal(&self, receiver: String) -> Result<bool, Error> {
        env::log("env Call in wasm create new proposal");
        println!("Call in wasm create new proposal {:?}", receiver);
        let account_id = env::ext::AccountId("vuki.testnet".to_string());
        let amount = 1;
        let proposal_id = Self::external()
            .propose()
            .transfer(account_id, amount)
            .send();

        println!("Create new proposal with id: {:?}", proposal_id);

        Ok(true)
    }

    pub fn approve_proposal(&mut self, proposal_id: env::ext::ProposalId) -> Result<bool, Error> {
        println!("Approve proposal: {:?}", proposal_id);
        let _ = Self::external().approve(proposal_id);
        Ok(true)
    }

    // Messages (discussion)
    pub fn get_proposal_messages(
        &self,
        // request: GetProposalMessagesRequest, I cannot to this??
        proposal_id: String,
    ) -> Result<Vec<Message>, Error> {
        env::log("env Get messages for proposal");

        let proposal_id = env::ext::ProposalId(Self::string_to_u8_32(proposal_id.as_str()));
        let res = &self.messages.get(&proposal_id).unwrap();

        match res {
            Some(messages) => Ok(messages.clone()),
            None => Ok(vec![]),
        }
    }

    pub fn send_proposal_messages(
        &mut self,
        // request: SendProposalMessageRequest, I cannot to this?? How to use camelCase?
        proposal_id: String,
        message: Message,
    ) -> Result<bool, Error> {
        env::log("env send_proposal_messages");

        let proposal_id = env::ext::ProposalId(Self::string_to_u8_32(proposal_id.as_str()));

        println!("Send message to proposal: {:?}", proposal_id);
        let proposal_messages = self.messages.get(&proposal_id).unwrap();
        match proposal_messages {
            Some(mut messages) => {
                messages.push(message);
                self.messages.insert(proposal_id, messages)?;
            }
            None => {
                let messages = vec![message];
                self.messages.insert(proposal_id, messages)?;
            }
        }
        Ok(true)
    }

    fn string_to_u8_32(s: &str) -> [u8; 32] {
        let mut array = [0u8; 32]; // Initialize array with 32 zeroes
        let bytes = s.as_bytes(); // Convert the string to bytes

        // Copy up to 32 bytes from the string slice into the array
        let len = bytes.len().min(32);
        array[..len].copy_from_slice(&bytes[..len]);

        array
    }
}
