#![allow(unused_crate_dependencies)]

use std::collections::BTreeMap;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{Application, Capability, ContextIdentity, Signed, SignerId};
use calimero_context_config::{
    ContextRequest, ContextRequestKind, Request, RequestKind, SystemRequest,
};
use ed25519_dalek::{Signer, SigningKey};
use near_workspaces::operations::Function;
use near_workspaces::types::{KeyType, SecretKey};
use near_workspaces::Contract;
use rand::{CryptoRng, Rng, RngCore};
use serde_json::json;
use tokio::{fs, time};

fn new_secret<R: CryptoRng + RngCore>(rng: &mut R) -> (SigningKey, SecretKey) {
    let rsk = SigningKey::generate(rng);

    let wsk =
        near_crypto::SecretKey::ED25519(near_crypto::ED25519SecretKey(rsk.to_keypair_bytes()))
            .to_string()
            .parse::<SecretKey>()
            .unwrap();

    (rsk, wsk)
}

#[tokio::test]
async fn main() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/calimero_context_config_near.wasm").await?;

    let mut rng = rand::thread_rng();

    let contract = worker
        .create_tla(
            "config-alt".parse()?,
            SecretKey::from_random(KeyType::SECP256K1),
        )
        .await?
        .into_result()?;

    let res = contract
        .batch(contract.id())
        .deploy(&wasm)
        .call(Function::new("init"))
        .transact()
        .await?
        .into_result()
        .expect_err("Secp256k1 should not be allowed");

    {
        let err = res.to_string();
        assert!(err.contains("pweety please, sign the the contract initialization transaction with an ed25519 key: decode error: insufficient length, found: 64, expected: 32"), "{}", err);
    }

    let (config_rsk, config_wsk) = new_secret(&mut rng);
    let config_pk = config_rsk.verifying_key();
    let config_id: Repr<ContextIdentity> = config_pk.to_bytes().rt()?;

    let contract = worker
        .create_tla("config".parse()?, config_wsk)
        .await?
        .into_result()?;

    let res = contract
        .batch(contract.id())
        .deploy(&wasm)
        .call(Function::new("init"))
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!("Contract initialized by `{}`", config_id)]
    );

    let contract = Contract::from_secret_key(
        contract.id().clone(),
        contract.secret_key().clone(),
        &worker,
    );

    let root_account = worker.root_account()?;

    let node1 = root_account
        .create_subaccount("node1")
        .transact()
        .await?
        .into_result()?;

    let node2 = root_account
        .create_subaccount("node3")
        .transact()
        .await?
        .into_result()?;

    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id = alice_cx_pk.to_bytes().rt()?;

    let bob_cx_sk = SigningKey::from_bytes(&rng.gen());
    let bob_cx_pk = bob_cx_sk.verifying_key();
    let bob_cx_id = bob_cx_pk.to_bytes().rt()?;

    let carol_cx_sk = SigningKey::from_bytes(&rng.gen());
    let carol_cx_pk = carol_cx_sk.verifying_key();
    let carol_cx_id = carol_cx_pk.to_bytes().rt()?;

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();
    let context_id = context_public.to_bytes().rt()?;

    let application_id = rng.gen::<[_; 32]>().rt()?;
    let blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application {
                            id: application_id,
                            blob: blob_id,
                            source: Default::default(),
                            metadata: Default::default(),
                        },
                    },
                });

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), [format!("Context `{}` added", context_id)]);

    let res = node2
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application {
                            id: application_id,
                            blob: blob_id,
                            source: Default::default(),
                            metadata: Default::default(),
                        },
                    },
                });

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .transact()
        .await?
        .raw_bytes()
        .expect_err("context should already exist");

    {
        let err = res.to_string();
        assert!(err.contains("context already exists"), "{}", err);
    }

    let res = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res: BTreeMap<Repr<SignerId>, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "identities": [],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 1);

    let alice_capabilities = res
        .get(&alice_cx_id.rt()?)
        .expect("alice should have capabilities");

    assert_eq!(
        alice_capabilities,
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

    assert_eq!(res, [alice_cx_id]);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::AddMembers {
                        members: vec![bob_cx_id].into(),
                    },
                });

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Added `{}` as a member of `{}`",
            bob_cx_id, context_id
        ),]
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

    assert_eq!(res, [alice_cx_id, bob_cx_id]);

    let res: BTreeMap<Repr<SignerId>, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "identities": [bob_cx_id],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 1);

    let bob_capabilities = res
        .get(&bob_cx_id.rt()?)
        .expect("bob should have capabilities");

    assert_eq!(bob_capabilities, &[]);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::AddMembers {
                        members: vec![carol_cx_id].into(),
                    },
                });

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
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

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::Grant {
                        capabilities: (vec![(bob_cx_id, Capability::ManageMembers)]).into(),
                    },
                });

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Granted `ManageMembers` to `{}` in `{}`",
            bob_cx_id, context_id
        )]
    );

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::AddMembers {
                        members: vec![carol_cx_id].into(),
                    },
                });

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
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
        .args_json(json!({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, [alice_cx_id, bob_cx_id, carol_cx_id]);

    let res: BTreeMap<Repr<SignerId>, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "identities": [],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 2);

    let alice_capabilities = res
        .get(&alice_cx_id.rt()?)
        .expect("alice should have capabilities");

    let bob_capabilities = res
        .get(&bob_cx_id.rt()?)
        .expect("bob should have capabilities");

    assert_eq!(res.get(&carol_cx_id.rt()?), None);

    assert_eq!(
        alice_capabilities,
        &[Capability::ManageApplication, Capability::ManageMembers]
    );

    assert_eq!(bob_capabilities, &[Capability::ManageMembers]);

    let new_application_id = rng.gen::<[_; 32]>().rt()?;
    let new_blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::UpdateApplication {
                        application: Application {
                            id: new_application_id,
                            blob: new_blob_id,
                            source: Default::default(),
                            metadata: Default::default(),
                        },
                    },
                });

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
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

    let res = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::UpdateApplication {
                        application: Application {
                            id: new_application_id,
                            blob: new_blob_id,
                            source: Default::default(),
                            metadata: Default::default(),
                        },
                    },
                });

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
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

    let res = contract
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, new_application_id);
    assert_eq!(res.blob, new_blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest {
                    context_id,
                    kind: ContextRequestKind::RemoveMembers {
                        members: vec![bob_cx_id].into(),
                    },
                });

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Removed `{}` from being a member of `{}`",
            bob_cx_id, context_id
        )]
    );

    let res: BTreeMap<Repr<SignerId>, Vec<Capability>> = contract
        .view("privileges")
        .args_json(json!({
            "context_id": context_id,
            "identities": [],
        }))
        .await?
        .json()?;

    assert_eq!(res.len(), 1);

    let alice_capabilities = res
        .get(&alice_cx_id.rt()?)
        .expect("alice should have capabilities");
    assert_eq!(res.get(&bob_cx_id.rt()?), None);
    assert_eq!(res.get(&carol_cx_id.rt()?), None);

    assert_eq!(
        alice_capabilities,
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

    assert_eq!(res, [alice_cx_id, carol_cx_id]);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::System(SystemRequest::SetValidityThreshold {
                    threshold_ms: 5_000,
                });

                Request::new(config_id.rt()?, kind)
            },
            |p| config_rsk.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), ["Set validity threshold to `5s`"]);

    let req = node1.call(contract.id(), "mutate").args_json(Signed::new(
        &{
            let kind = RequestKind::Context(ContextRequest {
                context_id,
                kind: ContextRequestKind::RemoveMembers {
                    members: vec![carol_cx_id].into(),
                },
            });

            Request::new(alice_cx_id.rt()?, kind)
        },
        |p| alice_cx_sk.sign(p),
    )?);

    time::sleep(time::Duration::from_secs(5)).await;

    let res = req
        .transact()
        .await?
        .raw_bytes()
        .expect_err("request should've expired");

    {
        let err = res.to_string();
        assert!(err.contains("request expired"), "{}", err);
    }

    let res: Vec<Repr<ContextIdentity>> = contract
        .view("members")
        .args_json(json!({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, [alice_cx_id, carol_cx_id]);

    Ok(())
}
