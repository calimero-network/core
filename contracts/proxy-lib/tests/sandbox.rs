use calimero_context_config::{
    repr::{Repr, ReprTransmute},
    types::{Application, ContextId, ContextIdentity, Signed},
    ContextRequest, ContextRequestKind, Request, RequestKind
};
use proxy_lib::{MultiSigRequest, MultiSigRequestAction, MultiSigRequestWithSigner, RequestId};
use ed25519_dalek::{Signer, SigningKey};
use near_workspaces::{network::Sandbox, types::NearToken, Account, Contract, Worker};
use rand::Rng;
use serde_json::json;
use eyre::Result;

const PROXY_CONTRACT_WASM: &str = "./res/proxy_lib.wasm";
const CONTEXT_CONFIG_WASM: &str = "../context-config/res/calimero_context_config_near.wasm";

#[tokio::test]
async fn test_fetch_members() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    // Deploy contracts
    let proxy_contract = deploy_contract(&worker, PROXY_CONTRACT_WASM).await?;
    let context_config_contract = deploy_contract(&worker, CONTEXT_CONFIG_WASM).await?;

    // Create alice account and node1 subaccount
    let alice = create_account_with_balance(&worker, "alice", 30).await?;
    let node1 = create_subaccount(&worker, "node1").await?;

    // Generate cryptographic identities
    let (alice_cx_id, context_id, signing_key) = generate_ids()?;

    // Add context via context-config contract
    add_context_to_config(
        &node1,
        &context_config_contract,
        context_id,
        alice_cx_id,
        signing_key
    ).await?;

    // Verify members in the context-config contract
    verify_members(
        &context_config_contract,
        context_id,
        alice_cx_id
    ).await?;

    // Initialize ProxyContract and test fetch_members
    initialize_proxy_contract(&proxy_contract, context_id, &context_config_contract).await?;
    test_fetch_members_call(&alice, &proxy_contract, alice_cx_id).await?;

    Ok(())
}

#[tokio::test]
async fn test_add_request_and_confirm() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    // Deploy contracts
    let proxy_contract = deploy_contract(&worker, PROXY_CONTRACT_WASM).await?;
    let context_config_contract = deploy_contract(&worker, CONTEXT_CONFIG_WASM).await?;

    // Create alice account and node1 subaccount
    let alice = create_account_with_balance(&worker, "alice", 30).await?;
    let node1 = create_subaccount(&worker, "node1").await?;

    // Generate cryptographic identities
    let (alice_cx_id, context_id, signing_key) = generate_ids()?;

    // Add context via context-config contract
    add_context_to_config(
        &node1,
        &context_config_contract,
        context_id,
        alice_cx_id,
        signing_key.clone()
    ).await?;

    // Initialize ProxyContract and test fetch_members
    initialize_proxy_contract(&proxy_contract, context_id, &context_config_contract).await?;
    let result = alice
    .call(proxy_contract.id(), "add_request_and_confirm")
    .args_json(json!({
        "request": Signed::new(
            &{
                let multi_sig_request = MultiSigRequest {
                    actions: vec![],
                    receiver_id: context_config_contract.id().clone(),
                };
                MultiSigRequestWithSigner {
                    signer_id: signing_key.verifying_key().to_bytes().rt()?,
                    request: multi_sig_request,
                }
            },
            |p| signing_key.sign(p),
        )?
    }))
    .max_gas()
    .transact()
    .await;
    assert!(result.is_ok());
    assert!(result?.json::<u32>()? == 0);
    Ok(())
}

async fn deploy_contract(worker: &Worker<Sandbox>, wasm_path: &str) -> Result<Contract> {
    let wasm = std::fs::read(wasm_path)?;
    let contract = worker.dev_deploy(&wasm).await?;
    Ok(contract)
}

async fn create_account_with_balance(worker: &Worker<Sandbox>, account_id: &str, balance: u128) -> Result<Account> {
    let root_account = worker.dev_create_account().await?;
    let account = root_account
        .create_subaccount(account_id)
        .initial_balance(NearToken::from_near(balance))
        .transact()
        .await?
        .into_result()?;
    Ok(account)
}

async fn create_subaccount(worker: &Worker<Sandbox>, subaccount_id: &str) -> Result<Account> {
    let root_account = worker.root_account()?;
    let subaccount = root_account
        .create_subaccount(subaccount_id)
        .transact()
        .await?
        .into_result()?;
    Ok(subaccount)
}

fn generate_ids() -> Result<(Repr<ContextIdentity>, Repr<ContextId>, SigningKey)> {
    let mut rng = rand::thread_rng();

    let alice_cx_sk = SigningKey::from_bytes(&rng.gen());
    let alice_cx_pk = alice_cx_sk.verifying_key();
    let alice_cx_id = alice_cx_pk.to_bytes().rt()?;

    let context_secret = SigningKey::from_bytes(&rng.gen());
    let context_public = context_secret.verifying_key();
    let context_id = context_public.to_bytes().rt()?;

    Ok((alice_cx_id, context_id, context_secret))
}

async fn add_context_to_config(
    node1: &Account,
    context_config_contract: &Contract,
    context_id: Repr<ContextId>,
    alice_cx_id: Repr<ContextIdentity>,
    signing_key: SigningKey
) -> Result<()> {
    let mut rng = rand::thread_rng();
    let application_id = rng.gen::<[_; 32]>().rt()?;
    let blob_id = rng.gen::<[_; 32]>().rt()?;

    let res = node1
        .call(context_config_contract.id(), "mutate")
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
            |p| signing_key.sign(p),
        )?)
        .transact()
        .await?
        .into_result()?;
    
    assert_eq!(res.logs(), [format!("Context `{}` added", context_id)]);
    Ok(())
}

async fn verify_members(
    context_config_contract: &Contract,
    context_id: Repr<ContextId>,
    alice_cx_id: Repr<ContextIdentity>
) -> Result<()> {
    let res: Vec<Repr<ContextIdentity>> = context_config_contract
        .view("members")
        .args_json(json!({
            "context_id": context_id,
            "offset": 0,
            "length": 10,
        }))
        .await?
        .json()?;

    assert_eq!(res, [alice_cx_id]);
    Ok(())
}

async fn initialize_proxy_contract(
    proxy_contract: &Contract,
    context_id: Repr<ContextId>,
    context_config_contract: &Contract
) -> Result<()> {
    let _ = proxy_contract
        .call("init")
        .args_json(json!({
            "context_id": context_id,
            "context_config_account_id": context_config_contract.id(),
        }))
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

async fn test_fetch_members_call(
    alice: &Account,
    proxy_contract: &Contract,
    alice_cx_id: Repr<ContextIdentity>
) -> Result<()> {
    let result: Vec<Repr<ContextIdentity>> = alice
        .call(proxy_contract.id(), "fetch_members")
        .max_gas()
        .transact()
        .await?
        .json()?;

    assert_eq!(result, [alice_cx_id]);
    Ok(())
}
