#![allow(unused_crate_dependencies)]

use std::collections::BTreeMap;
use std::time;

use context_config::Capability;
use context_config::ContextIdentity;
use ed25519_dalek::Signer;
use ed25519_dalek::SigningKey;
use near_sdk::AccountId;
use near_workspaces::types::NearToken;
use rand::Rng;
use serde_json::json;
use tokio::fs;

use context_config::{AddContextInput, Application, Repr, ReprBytes, SignedPayload};

pub trait ReprBytesExt: ReprBytes<DecodeBytes: AsRef<[u8]>> {
    fn from_bytes(bytes: Self::DecodeBytes) -> Self {
        ReprBytes::from_bytes(|b| {
            let len = bytes.as_ref().len();
            *b = bytes;
            Ok(len)
        })
        .unwrap()
    }
}

impl<T: ReprBytes<DecodeBytes: AsRef<[u8]>> + ?Sized> ReprBytesExt for T {}

#[tokio::test]
async fn main() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/context_config.wasm").await?;
    let contract = worker.dev_deploy(&wasm).await?;

    let root_account = worker.root_account()?;

    let mut rng = rand::thread_rng();

    let alice = root_account
        .create_subaccount("alice")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;
    let alice_cx_id = Repr::<ContextIdentity>::new(ReprBytesExt::from_bytes(rng.gen()));

    let bob = root_account
        .create_subaccount("bob")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;
    let bob_cx_id = Repr::<ContextIdentity>::new(ReprBytesExt::from_bytes(rng.gen()));

    let carol = root_account
        .create_subaccount("carol")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;
    let carol_cx_id = Repr::<ContextIdentity>::new(ReprBytesExt::from_bytes(rng.gen()));

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();

    let context_id = Repr::new(ReprBytesExt::from_bytes(context_public.to_bytes()));
    let application_id = Repr::new(ReprBytesExt::from_bytes(rng.gen()));
    let blob_id = Repr::new(ReprBytesExt::from_bytes(rng.gen()));

    let res = alice
        .call(contract.id(), "add_context")
        .args_json(SignedPayload::new(
            &AddContextInput {
                context_id,
                application: Application {
                    id: application_id,
                    blob: blob_id,
                    source: Default::default(),
                    metadata: Default::default(),
                },
                account_id: alice.id().clone(),
                timestamp_ms: time::SystemTime::now()
                    .duration_since(time::UNIX_EPOCH)?
                    .as_millis() as u64,
            },
            |p| context_secret.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), [format!("Context `{}` added", context_id)]);

    let res = alice
        .call(contract.id(), "add_context")
        .args_json(SignedPayload::new(
            &AddContextInput {
                context_id,
                application: Application {
                    id: application_id,
                    blob: blob_id,
                    source: Default::default(),
                    metadata: Default::default(),
                },
                account_id: alice.id().clone(),
                timestamp_ms: time::SystemTime::now()
                    .duration_since(time::UNIX_EPOCH)?
                    .as_millis() as u64,
            },
            |p| context_secret.sign(p),
        )?)
        .transact()
        .await?
        .raw_bytes()
        .expect_err("context should already exist");

    {
        let err = res.to_string();
        assert!(err.contains("Context already exists"), "{}", err);
    }

    let res: Application = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?
        .json()?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res: BTreeMap<AccountId, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "account_ids": [],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 1);
    let first = res.first_key_value().expect("expected one entry");

    assert_eq!(first.0, alice.id());
    assert_eq!(
        first.1,
        &[Capability::ManageApplication, Capability::ManageMembers]
    );

    let res: Vec<Repr<ContextIdentity>> = contract
        .view("members")
        .args_json(json!({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, []);

    let res = alice
        .call(contract.id(), "add_members")
        .args_json(json!({
            "context_id": context_id,
            "members": [alice_cx_id, bob_cx_id],
        }))
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [
            format!("Added `{}` as a member of `{}`", alice_cx_id, context_id),
            format!("Added `{}` as a member of `{}`", bob_cx_id, context_id),
        ]
    );

    let res: Vec<Repr<ContextIdentity>> = contract
        .view("members")
        .args_json(json! ({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, [alice_cx_id, bob_cx_id]);

    let res: BTreeMap<AccountId, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "account_ids": [bob.id()],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 1);
    let first = res.first_key_value().expect("expected one entry");

    assert_eq!(first.0, bob.id());
    assert_eq!(first.1, &[]);

    let res = bob
        .call(contract.id(), "add_members")
        .args_json(json!({
            "context_id": context_id,
            "members": [carol_cx_id],
        }))
        .transact()
        .await?
        .raw_bytes()
        .expect_err("bob lacks permissions");

    {
        let err = res.to_string();
        assert!(
            err.contains("unable to update member list: unauthorized access"),
            "{}",
            err
        );
    }

    let res = alice
        .call(contract.id(), "grant")
        .args_json(json!({
            "context_id": context_id,
            "capabilities": [(bob.id(), Capability::ManageMembers)],
        }))
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), [""; 0]);

    let res = bob
        .call(contract.id(), "add_members")
        .args_json(json!({
            "context_id": context_id,
            "members": [carol_cx_id],
        }))
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Added `{}` as a member of `{}`",
            carol_cx_id, context_id
        ),]
    );

    let res: Vec<Repr<ContextIdentity>> = contract
        .view("members")
        .args_json(json! ({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, [alice_cx_id, bob_cx_id, carol_cx_id]);

    let res: BTreeMap<AccountId, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "account_ids": [],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 2);

    let alice_capabilities = res.get(alice.id()).expect("alice should have capabilities");
    let bob_capabilities = res.get(bob.id()).expect("bob should have capabilities");

    assert_eq!(res.get(carol.id()), None);

    assert_eq!(
        alice_capabilities,
        &[Capability::ManageApplication, Capability::ManageMembers]
    );

    assert_eq!(bob_capabilities, &[Capability::ManageMembers]);

    let new_application_id = Repr::new(ReprBytesExt::from_bytes(rng.gen()));
    let new_blob_id = Repr::new(ReprBytesExt::from_bytes(rng.gen()));

    let res = bob
        .call(contract.id(), "update_application")
        .args_json(json!({
            "context_id": context_id,
            "application": Application {
                id: new_application_id,
                blob: new_blob_id,
                source: Default::default(),
                metadata: Default::default(),
            },
        }))
        .transact()
        .await?
        .raw_bytes()
        .expect_err("bob lacks permissions");

    {
        let err = res.to_string();
        assert!(
            err.contains("unable to update application: unauthorized access"),
            "{}",
            err
        );
    }

    let res: Application = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?
        .json()?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res = alice
        .call(contract.id(), "update_application")
        .args_json(json!({
            "context_id": context_id,
            "application": Application {
                id: new_application_id,
                blob: new_blob_id,
                source: Default::default(),
                metadata: Default::default(),
            },
        }))
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Updated application `{}` -> `{}`",
            application_id, new_application_id
        )]
    );

    let res: Application = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?
        .json()?;

    assert_eq!(res.id, new_application_id);
    assert_eq!(res.blob, new_blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    Ok(())
}
