#![allow(unused_crate_dependencies)]
use std::collections::BTreeMap;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{
    Application, Capability, ContextIdentity, Revision, Signed, SignerId,
};
use calimero_context_config::{
    ContextRequest, ContextRequestKind, Proposal, ProposalAction, ProposalWithApprovals,
    ProxyMutateRequest, Request, RequestKind, SystemRequest,
};
use ed25519_dalek::{Signer, SigningKey};
use eyre::Ok;
use near_sdk::AccountId;
use near_workspaces::types::NearToken;
use rand::Rng;
use serde_json::json;
use tokio::{fs, time};

#[tokio::test]
async fn main() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/calimero_context_config_near.wasm").await?;

    let mut rng = rand::thread_rng();

    let contract = worker.dev_deploy(&wasm).await?;

    let root_account = worker.root_account()?;

    let node1 = root_account
        .create_subaccount("node1")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;

    let node2 = root_account
        .create_subaccount("node2")
        .initial_balance(NearToken::from_near(30))
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
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .raw_bytes()
        .expect_err("context creation should fail");

    {
        let err = res.to_string();
        assert!(
            err.contains("context addition must be signed by the context itself"),
            "{}",
            err
        );
    }

    let new_proxy_wasm = fs::read("../proxy-lib/res/proxy_lib.wasm").await?;
    let _test = contract
        .call("set_proxy_code")
        .args(new_proxy_wasm)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    // Assert context creation
    let expected_log = format!("Context `{}` added", context_id);
    assert!(res.logs().iter().any(|log| log == &expected_log));

    let res = node2
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

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
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::AddMembers {
                        members: vec![bob_cx_id].into(),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 1);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::AddMembers {
                        members: vec![carol_cx_id].into(),
                    },
                ));

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 1);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Grant {
                        capabilities: (vec![(bob_cx_id, Capability::ManageMembers)]).into(),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 1);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::AddMembers {
                        members: vec![carol_cx_id].into(),
                    },
                ));

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 2);

    let new_application_id = rng.gen::<[_; 32]>().rt()?;
    let new_blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateApplication {
                        application: Application::new(
                            new_application_id,
                            new_blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(bob_cx_id.rt()?, kind)
            },
            |p| bob_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 0);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 2);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateApplication {
                        application: Application::new(
                            new_application_id,
                            new_blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        res.logs(),
        [format!(
            "Updated application for context `{}` from `{}` to `{}`",
            context_id, application_id, new_application_id
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 1);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 2);

    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::RemoveMembers {
                        members: vec![bob_cx_id].into(),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
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

    let res = contract
        .view("application_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 1);

    let res = contract
        .view("members_revision")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Revision = serde_json::from_slice(&res.result)?;

    assert_eq!(res, 3);

    let res = contract
        .call("set")
        .args_json(SystemRequest::SetValidityThreshold {
            threshold_ms: 5_000,
        })
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), ["Set validity threshold to `5s`"]);

    let req = node1.call(contract.id(), "mutate").args_json(Signed::new(
        &{
            let kind = RequestKind::Context(ContextRequest::new(
                context_id,
                ContextRequestKind::RemoveMembers {
                    members: vec![carol_cx_id].into(),
                },
            ));

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

    let res = contract
        .call("erase")
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.logs().contains(&"Erasing contract"), "{:?}", res.logs());

    let state = contract.view_state().await?;

    assert_eq!(state.len(), 1);
    assert_eq!(state.get(&b"STATE"[..]).map(|v| v.len()), Some(37));

    Ok(())
}

#[ignore]
#[tokio::test]
async fn migration() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let wasm_v0 = fs::read("res/calimero_context_config_near_v0.wasm").await?;
    let wasm_v1 = fs::read("res/calimero_context_config_near_v1.wasm").await?;

    let mut rng = rand::thread_rng();

    let contract_v0 = worker.dev_deploy(&wasm_v0).await?;

    let root_account = worker.root_account()?;

    let node1 = root_account
        .create_subaccount("node1")
        .transact()
        .await?
        .into_result()?;

    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id = alice_cx_pk.to_bytes().rt()?;

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();
    let context_id = context_public.to_bytes().rt()?;

    let application_id = rng.gen::<[_; 32]>().rt()?;
    let blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(contract_v0.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: alice_cx_id,
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;

    assert_eq!(res.logs(), [format!("Context `{}` added", context_id)]);

    let res = contract_v0
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let contract_v1 = contract_v0
        .as_account()
        .deploy(&wasm_v1)
        .await?
        .into_result()?;

    let res = contract_v1
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await
        .expect_err("should've failed");

    {
        let err = format!("{:?}", res);
        assert!(err.contains("Cannot deserialize element"), "{}", err);
    }

    let migration = contract_v1
        .call("migrate")
        .transact()
        .await?
        .into_result()?;

    dbg!(migration.logs());

    let res = contract_v1
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    Ok(())
}

#[tokio::test]
async fn test_deploy() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/calimero_context_config_near.wasm").await?;
    let mut rng = rand::thread_rng();

    let contract = worker.dev_deploy(&wasm).await?;
    let root_account = worker.root_account()?;
    let node1 = root_account
        .create_subaccount("node1")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;

    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id: ContextIdentity = alice_cx_pk.to_bytes().rt()?;

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();
    let context_id = context_public.to_bytes().rt()?;

    // Set proxy code
    let new_proxy_wasm = fs::read("../proxy-lib/res/proxy_lib.wasm").await?;
    let _test = contract
        .call("set_proxy_code")
        .args(new_proxy_wasm)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let application_id = rng.gen::<[_; 32]>().rt()?;
    let blob_id = rng.gen::<[_; 32]>().rt()?;

    // Call mutate without deposit since contract uses its own balance
    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: Repr::new(alice_cx_id),
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    dbg!(res.total_gas_burnt);

    // Assert context creation
    let expected_log = format!("Context `{}` added", context_id);
    assert!(res.logs().iter().any(|log| log == &expected_log));

    // Verify the proxy contract was deployed
    let proxy_address: AccountId = contract
        .view("proxy_contract")
        .args_json(json!({
            "context_id": context_id
        }))
        .await?
        .json()?;

    // Assert the proxy address is a subaccount of the contract
    assert!(
        proxy_address.to_string() == format!("0.{}", contract.id()),
        "Proxy address '{}' should be exactly '0.{}'",
        proxy_address,
        contract.id()
    );

    // Call the update function
    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateProxyContract,
                ));

                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(
        res.logs()
            .iter()
            .any(|log| log.contains("Successfully updated proxy contract")),
        "Expected success message in logs"
    );

    // Create proposal
    let proposal_id = rand::thread_rng()
        .gen::<[_; 32]>()
        .rt()
        .expect("infallible conversion");
    let actions = vec![ProposalAction::ExternalFunctionCall {
        receiver_id: contract.id().to_string(),
        method_name: "increment".to_string(),
        args: "[]".to_string(),
        deposit: 0,
        gas: 1_000_000_000_000,
    }];

    let request = ProxyMutateRequest::Propose {
        proposal: Proposal {
            id: proposal_id,
            author_id: alice_cx_id.rt()?,
            actions,
        },
    };
    let signed = Signed::new(&request, |p| alice_cx_sk.sign(p))?;

    let res = node1
        .call(&proxy_address, "mutate")
        .args_json(signed)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    // Assert proposal creation result
    let proposal_result = res.json::<ProposalWithApprovals>()?;
    assert_eq!(proposal_result.num_approvals, 1);
    let created_proposal_id = proposal_result.proposal_id;
    // Verify proposals list
    let proposals: Vec<Proposal> = worker
        .view(&proxy_address, "proposals")
        .args_json(json!({
            "offset": 0,
            "length": 10
        }))
        .await?
        .json()?;

    assert_eq!(proposals.len(), 1, "Should have exactly one proposal");
    let created_proposal = &proposals[0];
    assert_eq!(created_proposal.id, created_proposal_id);
    assert_eq!(created_proposal.author_id, alice_cx_id.rt()?);
    assert_eq!(created_proposal.actions.len(), 1);

    if let ProposalAction::ExternalFunctionCall {
        receiver_id,
        method_name,
        args,
        deposit,
        gas,
    } = &created_proposal.actions[0]
    {
        assert_eq!(receiver_id, contract.id());
        assert_eq!(method_name, "increment");
        assert_eq!(args, "[]");
        assert_eq!(*deposit, 0);
        assert_eq!(*gas, 1_000_000_000_000);
    } else {
        panic!("Expected ExternalFunctionCall action");
    }

    // Verify single proposal query
    let single_proposal: Option<Proposal> = worker
        .view(&proxy_address, "proposal")
        .args_json(json!({
            "proposal_id": proposal_id
        }))
        .await?
        .json()?;

    assert!(
        single_proposal.is_some(),
        "Should be able to get single proposal"
    );
    assert_eq!(single_proposal.unwrap().id, created_proposal_id);

    Ok(())
}

#[tokio::test]
async fn test_storage_usage_matches_code_size() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = fs::read("res/calimero_context_config_near.wasm").await?;
    let mut rng = rand::thread_rng();

    let contract = worker.dev_deploy(&wasm).await?;
    let root_account = worker.root_account()?;
    let node1 = root_account
        .create_subaccount("node1")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;

    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id: ContextIdentity = alice_cx_pk.to_bytes().rt()?;

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();
    let context_id = context_public.to_bytes().rt()?;

    let bigger_proxy_wasm = fs::read("../proxy-lib/res/proxy_lib_fat.wasm").await?;
    let smaller_proxy_wasm = fs::read("../proxy-lib/res/proxy_lib.wasm").await?;

    println!("Config contract: {}", contract.id());
    let config_balance = worker.view_account(&contract.id()).await?.balance;
    println!("Config contract balance: {}", config_balance);

    // Set initial proxy code
    let res = contract
        .call("set_proxy_code")
        .args(bigger_proxy_wasm.clone())
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.failures().is_empty(), "{:#?}", res.failures());

    let application_id = rng.gen::<[_; 32]>().rt()?;
    let blob_id = rng.gen::<[_; 32]>().rt()?;

    let node1_balance = worker.view_account(&node1.id()).await?.balance;

    // Deploy proxy contract
    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id: Repr::new(alice_cx_id),
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));
                Request::new(context_id.rt()?, kind)
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    // Verify proxy contract deployment
    let expected_log = format!("Context `{}` added", context_id);
    assert!(res.logs().iter().any(|log| log == &expected_log));

    let node1_balance_after = worker.view_account(&node1.id()).await?.balance;

    let diff = node1_balance.saturating_sub(node1_balance_after);
    let node1_balance = node1_balance_after;

    assert!(
        diff < NearToken::from_millinear(15),
        "Node1 balance should not be reduced by more than 15 milliNEAR, but was reduced by {}",
        diff
    );

    let proxy_address: AccountId = contract
        .view("proxy_contract")
        .args_json(json!({ "context_id": context_id }))
        .await?
        .json()?;

    println!("Proxy address: {}", proxy_address);
    let proxy_balance = worker.view_account(&proxy_address).await?.balance;
    println!("Proxy balance: {}", proxy_balance);

    // Get initial measurements
    let initial_outcome = worker.view_account(&proxy_address).await?;
    let initial_storage = initial_outcome.storage_usage;
    let initial_code_size = bigger_proxy_wasm.len() as u64;
    let initial_balance = initial_outcome.balance;

    println!("Initial storage usage: {}", initial_storage);
    println!("Initial WASM size: {}", initial_code_size);
    println!("Initial Balance: {}", initial_balance);

    let res = contract
        .call("set_proxy_code")
        .args(smaller_proxy_wasm.clone())
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.failures().is_empty(), "{:#?}", res.failures());

    // Update proxy contract
    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateProxyContract,
                ));
                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.failures().is_empty(), "{:#?}", res.failures());

    let node1_balance_after = worker.view_account(&node1.id()).await?.balance;

    let diff = node1_balance.saturating_sub(node1_balance_after);
    let node1_balance = node1_balance_after;

    assert!(
        diff < NearToken::from_millinear(10),
        "Node1 balance should not be reduced by more than 10 milliNEAR, but was reduced by {}",
        diff
    );

    // Get intermediate measurements
    let intermediate_outcome = worker.view_account(&proxy_address).await?;
    let intermediate_storage = intermediate_outcome.storage_usage;
    let intermediate_code_size = smaller_proxy_wasm.len() as u64;
    let intermediate_balance = intermediate_outcome.balance;

    println!("Intermediate storage usage: {}", intermediate_storage);
    println!("Intermediate WASM size: {}", intermediate_code_size);
    println!("Intermediate Balance: {}", intermediate_balance);

    // Calculate raw differences (can be negative)
    let storage_change = intermediate_storage as i64 - initial_storage as i64;
    let code_change = intermediate_code_size as i64 - initial_code_size as i64;
    let intermediate_balance_change =
        intermediate_balance.as_yoctonear() as i128 - initial_balance.as_yoctonear() as i128;
    let intermediate_balance_change_is_negative = intermediate_balance_change.is_negative();
    let intermediate_balance_change =
        NearToken::from_yoctonear(intermediate_balance_change.unsigned_abs());

    println!("Storage change: {:+}", storage_change);
    println!("Code change: {:+}", code_change);
    println!(
        "Balance change: {:+} (negative: {})",
        intermediate_balance_change, intermediate_balance_change_is_negative
    );

    assert!(intermediate_balance_change_is_negative);

    assert_eq!(
        storage_change, code_change,
        "Storage change ({:+}) should exactly match code size change ({:+})",
        storage_change, code_change
    );

    let res = contract
        .call("set_proxy_code")
        .args(bigger_proxy_wasm.clone())
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.failures().is_empty(), "{:#?}", res.failures());

    // Update proxy contract
    let res = node1
        .call(contract.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateProxyContract,
                ));
                Request::new(alice_cx_id.rt()?, kind)
            },
            |p| alice_cx_sk.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(res.failures().is_empty(), "{:#?}", res.failures());

    let node1_balance_after = worker.view_account(&node1.id()).await?.balance;

    let diff = node1_balance.saturating_sub(node1_balance_after);

    assert!(
        diff < NearToken::from_millinear(10),
        "Node1 balance should not be reduced by more than 10 milliNEAR, but was reduced by {}",
        diff
    );

    // Get final measurements
    let final_outcome = worker.view_account(&proxy_address).await?;
    let final_storage = final_outcome.storage_usage;
    let final_code_size = bigger_proxy_wasm.len() as u64;
    let final_balance = final_outcome.balance;

    println!("Final storage usage: {}", final_storage);
    println!("Final WASM size: {}", final_code_size);
    println!("Final Balance: {}", final_balance);

    // Calculate raw differences (can be negative)
    let storage_change = final_storage as i64 - intermediate_storage as i64;
    let code_change = final_code_size as i64 - intermediate_code_size as i64;
    let final_balance_change =
        final_balance.as_yoctonear() as i128 - intermediate_balance.as_yoctonear() as i128;
    let final_balance_change_is_negative = final_balance_change.is_negative();
    let final_balance_change = NearToken::from_yoctonear(final_balance_change.unsigned_abs());

    println!("Storage change: {:+}", storage_change);
    println!("Code change: {:+}", code_change);
    println!(
        "Balance change: {:+} (negative: {})",
        final_balance_change, final_balance_change_is_negative
    );

    assert!(!final_balance_change_is_negative);

    let diff = final_balance_change
        .as_yoctonear()
        .abs_diff(intermediate_balance_change.as_yoctonear());

    assert!(
        NearToken::from_yoctonear(diff) < NearToken::from_millinear(1),
        "Balance change should be within a milliNEAR"
    );

    assert_eq!(
        storage_change, code_change,
        "Storage change ({:+}) should exactly match code size change ({:+})",
        storage_change, code_change
    );

    let config_balance = worker.view_account(&contract.id()).await?.balance;
    println!("Config contract balance: {}", config_balance);

    Ok(())
}
