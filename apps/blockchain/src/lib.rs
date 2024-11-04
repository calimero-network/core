use calimero_sdk::borsh::BorshSerialize;
use calimero_sdk::env::{self};
use calimero_sdk::types::Error;
use calimero_sdk::{app, borsh};
use calimero_storage::entities::Element;
use calimero_storage::AtomicUnit;

#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[borsh(crate = "calimero_sdk::borsh")]
#[type_id(1)]
pub struct CreateProposalRequest {
    proposal_id: String,
    author: Option<String>,
    // status: ProposalStatus,
    #[storage]
    storage: Element,
}

#[app::state(emits = Event)]
#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[root]
#[type_id(1)]
pub struct AppState {
    count: u32,
    #[storage]
    storage: Element,

    messages: Vec<Message>,
    // messages: Vec<env::ext::ProposalId>,
    // messages: Map<String, Vec<Message>>,
}

#[app::event]
pub enum Event {
    ProposalCreated(),
}

// #[derive(AtomicUnit, BorshDeserialize, BorshSerialize, Default, Serialize)]
#[derive(AtomicUnit, BorshSerialize)]
#[type_id(1)]
pub struct Message {
    id: String,
    proposal_id: String,
    author: String,
    text: String,
    created_at: String,

    #[storage]
    storage: Element,
}

#[app::logic]
impl AppState {
    #[app::init]
    pub fn init() -> AppState {
        AppState {
            count: 0,
            storage: Element::root(),
            messages: Vec::new(),
        }
    }

    pub fn create_new_proposal(&mut self, _request: CreateProposalRequest) -> Result<bool, Error> {
        // let proposal_id = Blockchain::create_proposal("transfer", "xabi.near", 999999);
        // let enhanced_proposal = Proposal {
        //     proposal_id,
        //     title,
        //     description
        // };
        // storage.save(enhanced_proposal)

        let account_id = env::ext::AccountId("cali.near".to_string());
        let amount = 1;
        let proposal_id = Self::external()
            .propose()
            .transfer(account_id, amount)
            .send();

        println!("Proposal ID: {:?}", proposal_id);

        Ok(true)
    }

    pub fn approve_proposal(&mut self, _proposal_id: String) -> Result<bool, Error> {
        // let proposal = storage.get_proposal(proposal_id);
        // let vote = Vote {
        //     proposal_id,
        //     voter_id: "xabi.near",
        //     vote_type: VoteType::Accept(),
        //     voted_at: 1234567890,
        // };
        // storage.save(vote)
        Ok(true)
    }

    // Messages (discussion)
    pub fn get_proposal_messages(&self, _proposal_id: String) -> Result<&Vec<Message>, Error> {
        let res = &self.messages;
        Ok(res)
    }
    // pub fn send_message(proposal_id: String, message: Message) -> bool {
    //     true
    // }
}
