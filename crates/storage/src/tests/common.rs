use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;
use ed25519_dalek::{Signer, SigningKey};
use velcro::btree_map;

use crate::action::Action;
use crate::address::Id;
use crate::entities::{
    AtomicUnit, ChildInfo, Collection, Data, Element, Metadata, SignatureData, StorageType,
};
use crate::env;
use crate::interface::MainInterface;

/// For tests against empty data structs.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct EmptyData {
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Data for EmptyData {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// A simple page with a title, and paragraphs as children.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Page {
    /// The title of the page.
    pub title: String,
    /// Collection of paragraphs in this page.
    pub paragraphs: Paragraphs,
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Page {
    /// Creates a new page with a title from an existing element.
    pub fn new_from_element(title: &str, element: Element) -> Self {
        Self {
            title: title.to_owned(),
            paragraphs: Paragraphs::new(),
            storage: element,
        }
    }
}

impl AtomicUnit for Page {}

impl Data for Page {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        btree_map! {
            "Paragraphs".to_owned(): MainInterface::child_info_for(self.id()).unwrap_or_default(),
        }
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// A simple paragraph with text. No children. Belongs to a page.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Paragraph {
    /// The text content of the paragraph.
    pub text: String,
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Paragraph {
    /// Creates a new paragraph with text, from an existing element.
    pub fn new_from_element(text: &str, element: Element) -> Self {
        Self {
            text: text.to_owned(),
            storage: element,
        }
    }
}

impl AtomicUnit for Paragraph {}

impl Data for Paragraph {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// A collection of paragraphs for a page.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
pub struct Paragraphs;

impl Paragraphs {
    /// Creates a new paragraph collection.
    pub fn new() -> Self {
        Self {}
    }
}

impl Collection for Paragraphs {
    type Child = Paragraph;
}

/// A simple person example struct. No children.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Person {
    /// The name of the person.
    pub name: String,
    /// The age of the person.
    pub age: u8,
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Data for Person {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// Helper to create a test keypair and public key.
pub fn create_test_keypair() -> (SigningKey, PublicKey) {
    let mut seed = [0u8; 32];
    env::random_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let public_key = PublicKey::from(*verifying_key.as_bytes());
    (signing_key, public_key)
}

/// Helper to sign an action with the given signing key.
pub fn sign_action(action: &Action, signing_key: &SigningKey) -> [u8; 64] {
    let payload = action.payload_for_signing();
    let signature = signing_key.sign(&payload);
    signature.to_bytes()
}

/// Helper to create a User storage action (Add) with proper signature.
pub fn create_signed_user_add_action(
    signing_key: &SigningKey,
    owner: PublicKey,
    id: Id,
    data: Vec<u8>,
    nonce: u64,
) -> Action {
    let timestamp = env::time_now();

    // Create metadata with placeholder signature
    let metadata = Metadata {
        created_at: timestamp,
        updated_at: timestamp.into(),
        storage_type: StorageType::User {
            owner,
            signature_data: Some(SignatureData {
                signature: [0; 64], // Placeholder
                nonce,
            }),
        },
    };

    // Create action for signing
    let mut action = Action::Add {
        id,
        data,
        ancestors: vec![],
        metadata,
    };

    // Sign and update action
    let signature = sign_action(&action, signing_key);

    if let Action::Add {
        ref mut metadata, ..
    } = action
    {
        if let StorageType::User {
            ref mut signature_data,
            ..
        } = metadata.storage_type
        {
            *signature_data = Some(SignatureData { signature, nonce });
        }
    }

    action
}

/// Helper to create a User storage Update action with proper signature.
pub fn create_signed_user_update_action(
    signing_key: &SigningKey,
    owner: PublicKey,
    id: Id,
    data: Vec<u8>,
    nonce: u64,
    created_at: u64,
) -> Action {
    let timestamp = env::time_now();

    let metadata = Metadata {
        created_at,
        updated_at: timestamp.into(),
        storage_type: StorageType::User {
            owner,
            signature_data: Some(SignatureData {
                signature: [0; 64],
                nonce,
            }),
        },
    };

    let mut action = Action::Update {
        id,
        data,
        ancestors: vec![],
        metadata,
    };

    let signature = sign_action(&action, signing_key);

    if let Action::Update {
        ref mut metadata, ..
    } = action
    {
        if let StorageType::User {
            ref mut signature_data,
            ..
        } = metadata.storage_type
        {
            *signature_data = Some(SignatureData { signature, nonce });
        }
    }

    action
}
