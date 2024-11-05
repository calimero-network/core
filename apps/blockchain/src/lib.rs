use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env::{self};
use calimero_sdk::types::Error;
use calimero_storage::collections::UnorderedMap;
use calimero_storage::entities::Element;
use calimero_storage::AtomicUnit;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, PartialOrd, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct CreateProposalRequest {
    proposal_id: String,
    author: Option<String>,
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

    pub fn create_new_proposal(&mut self, _request: CreateProposalRequest) -> Result<bool, Error> {
        let account_id = env::ext::AccountId("cali.near".to_string());
        let amount = 1;
        let proposal_id = Self::external()
            .propose()
            .transfer(account_id, amount)
            .send();

        println!("Create new proposal with id: {:?}", proposal_id);

        Ok(true)
    }

    pub fn approve_proposal(&mut self, _proposal_id: env::ext::ProposalId) -> Result<bool, Error> {
        // Self::external()
        Ok(true)
    }

    // Messages (discussion)
    pub fn get_proposal_messages(
        &self,
        proposal_id: env::ext::ProposalId,
    ) -> Result<Vec<Message>, Error> {
        let res = &self.messages.get(&proposal_id).unwrap();

        match res {
            Some(messages) => Ok(messages.clone()),
            None => Ok(vec![]),
        }
    }

    pub fn send_message(
        &mut self,
        proposal_id: env::ext::ProposalId,
        message: Message,
    ) -> Result<bool, Error> {
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
}
