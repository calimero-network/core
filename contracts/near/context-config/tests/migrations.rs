#![allow(unused_crate_dependencies)]

use std::time;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{Application, Signed, SignerId};
use calimero_context_config::{ContextRequest, ContextRequestKind, Request, RequestKind};
use ed25519_dalek::{Signer, SigningKey};
use near_sdk::serde::Serialize;
use near_sdk::NearToken;
use rand::Rng;
use serde_json::json;
use tokio::fs;

#[cfg_attr(not(feature = "01_guard_revisions"), ignore)]
#[tokio::test]
async fn migration_revision_guard() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let wasm_v0 =
        fs::read("res/calimero_context_config_near_migration_revision_guard_pre.wasm").await?;
    let wasm_v1 =
        fs::read("res/calimero_context_config_near_migration_revision_guard_post.wasm").await?;

    let mut rng = rand::thread_rng();

    let contract_v0 = worker.dev_deploy(&wasm_v0).await?;

    let context_proxy_blob =
        fs::read("../context-proxy/res/calimero_context_proxy_near.wasm").await?;

    let _ignored = contract_v0
        .call("set_proxy_code")
        .args(context_proxy_blob)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let root_account = worker.root_account()?;

    let node1 = root_account
        .create_subaccount("node1")
        .initial_balance(NearToken::from_near(1))
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

                Request::new(context_id.rt()?, kind, 0)
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(
        res.logs()
            .contains(&format!("Context `{}` added", context_id).as_str()),
        "{:?}",
        res.logs()
    );

    let res = contract_v0
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, application_id);
    assert_eq!(res.blob, blob_id);
    assert_eq!(res.size, 0);
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
    assert_eq!(res.size, 0);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    Ok(())
}

#[cfg_attr(not(feature = "02_nonces"), ignore)]
#[tokio::test]
async fn migration_member_nonces() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;

    // testnet block 185_028_495
    let wasm_v0 =
        fs::read("res/calimero_context_config_near_migration_member_nonces_pre.wasm").await?;
    let wasm_v1 =
        fs::read("res/calimero_context_config_near_migration_member_nonces_post.wasm").await?;

    let mut rng = rand::thread_rng();

    let contract_v0 = worker.dev_deploy(&wasm_v0).await?;

    let context_proxy_blob =
        fs::read("../context-proxy/res/calimero_context_proxy_near.wasm").await?;

    let _ignored = contract_v0
        .call("set_proxy_code")
        .args(context_proxy_blob)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let root_account = worker.root_account()?;

    let node1 = root_account
        .create_subaccount("node1")
        .initial_balance(NearToken::from_near(1))
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

    #[derive(Debug, Serialize)]
    #[serde(crate = "near_sdk::serde")]
    #[serde(rename_all = "camelCase")]
    struct OldRequest<'a> {
        signer_id: Repr<SignerId>,
        timestamp_ms: u64,

        #[serde(flatten)]
        kind: RequestKind<'a>,
    }

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

                OldRequest {
                    kind,
                    signer_id: context_id.rt()?,
                    timestamp_ms: time::SystemTime::now()
                        .duration_since(time::UNIX_EPOCH)
                        .expect("system time is before epoch?")
                        .as_millis() as _,
                }
            },
            |p| context_secret.sign(p),
        )?)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert!(
        res.logs()
            .contains(&format!("Context `{}` added", context_id).as_str()),
        "{:?}",
        res.logs()
    );

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
    assert_eq!(res.size, 0);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res: Option<u64> = contract_v1
        .view("fetch_nonce")
        .args_json(json!({ "context_id": context_id, "member_id": alice_cx_id }))
        .await?
        .json()?;

    let nonce = res.expect("nonce not found");

    assert_eq!(nonce, 0);

    let new_application_id = rng.gen::<[_; 32]>().rt()?;
    let new_blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(contract_v1.id(), "mutate")
        .args_json(Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateApplication {
                        application: Application::new(
                            new_application_id,
                            new_blob_id,
                            1,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));

                Request::new(alice_cx_id.rt()?, kind, nonce)
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

    let res = contract_v1
        .view("application")
        .args_json(json!({ "context_id": context_id }))
        .await?;

    let res: Application<'_> = serde_json::from_slice(&res.result)?;

    assert_eq!(res.id, new_application_id);
    assert_eq!(res.blob, new_blob_id);
    assert_eq!(res.size, 1);
    assert_eq!(res.source, Default::default());
    assert_eq!(res.metadata, Default::default());

    let res: Option<u64> = contract_v1
        .view("fetch_nonce")
        .args_json(json!({ "context_id": context_id, "member_id": alice_cx_id }))
        .await?
        .json()?;

    let nonce = res.expect("nonce not found");

    assert_eq!(nonce, 1);

    Ok(())
}
