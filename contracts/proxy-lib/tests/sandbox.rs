use calimero_context_config::repr::ReprTransmute;
use common::{
    config_helper::ConfigContractHelper, counter_helper::CounterContracttHelper,
    proxy_lib_helper::ProxyContractHelper,
};
use ed25519_dalek::SigningKey;
use eyre::Result;
use near_sdk::{json_types::Base64VecU8, Gas, NearToken};
use near_workspaces::{network::Sandbox, Account, Worker};
use proxy_lib::{Proposal, ProposalAction, ProposalWithApprovals};

mod common;

async fn setup_test(
    worker: &Worker<Sandbox>,
) -> Result<(
    ConfigContractHelper,
    ProxyContractHelper,
    Account,
    SigningKey,
    SigningKey,
)> {
    let config_helper = ConfigContractHelper::new(&worker).await?;
    let proxy_helper =
        ProxyContractHelper::new(&worker, config_helper.clone().config_contract).await?;

    let relayer_account = common::create_account_with_balance(&worker, "account", 10).await?;
    // This account is only used to deploy the proxy contract
    let developer_account = common::create_account_with_balance(&worker, "alice", 10).await?;

    let context_sk = common::generate_keypair()?;
    let alice_sk: SigningKey = common::generate_keypair()?;

    let _res = config_helper
        .add_context_to_config(&relayer_account, &context_sk, &alice_sk)
        .await?
        .into_result()?;

    let _res = proxy_helper
        .initialize(&developer_account, &context_sk.verifying_key().rt()?)
        .await;

    Ok((
        config_helper,
        proxy_helper,
        relayer_account,
        context_sk,
        alice_sk,
    ))
}

#[tokio::test]
async fn test_create_proposal() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    let proposal = proxy_helper.create_proposal(
        &alice_sk,
        &config_helper.config_contract.as_account(),
        vec![],
    )?;

    let res: ProposalWithApprovals = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res.proposal_id, 0);
    assert_eq!(res.num_approvals, 1);

    Ok(())
}

#[tokio::test]
async fn test_create_proposal_by_non_member() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (config_helper, proxy_helper, relayer_account, _context_sk, _alice_sk) =
        setup_test(&worker).await?;

    // Bob is not a member of the context
    let bob_sk: SigningKey = common::generate_keypair()?;

    let proposal: calimero_context_config::types::Signed<Proposal> = proxy_helper.create_proposal(
        &bob_sk,
        &config_helper.config_contract.as_account(),
        vec![],
    )?;

    let res = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result();

    let error = res.expect_err("Expected an error from the contract");
    assert!(error.to_string().contains("Not a context member"));

    let view_proposal: ProposalWithApprovals = proxy_helper
        .view_proposal_confirmations(&relayer_account, &0)
        .await?
        .json()?;
    assert_eq!(view_proposal.num_approvals, 0);
    Ok(())
}

#[tokio::test]
async fn test_create_multiple_proposals() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let (config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    let proposal: calimero_context_config::types::Signed<Proposal> = proxy_helper.create_proposal(
        &alice_sk,
        &config_helper.config_contract.as_account(),
        vec![],
    )?;

    let _res = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result();

    let res: ProposalWithApprovals = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res.proposal_id, 1);
    assert_eq!(res.num_approvals, 1);

    Ok(())
}

#[tokio::test]
async fn test_create_proposal_and_approve_by_member() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let (config_helper, proxy_helper, relayer_account, context_sk, alice_sk) =
        setup_test(&worker).await?;

    // Add Bob as a context member
    let bob_sk: SigningKey = common::generate_keypair()?;
    let _res = config_helper
        .add_members(&relayer_account, &alice_sk, &[bob_sk.clone()], &context_sk)
        .await?
        .into_result()?;

    let proposal: calimero_context_config::types::Signed<Proposal> = proxy_helper.create_proposal(
        &alice_sk,
        &config_helper.config_contract.as_account(),
        vec![],
    )?;

    let res: ProposalWithApprovals = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    let res2: ProposalWithApprovals = proxy_helper
        .approve_proposal(&relayer_account, &bob_sk, &res.proposal_id)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res2.proposal_id, 0);
    assert_eq!(res2.num_approvals, 2);

    Ok(())
}

#[tokio::test]
async fn test_create_proposal_and_approve_by_non_member() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let (config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    // Bob is not a member of the context
    let bob_sk: SigningKey = common::generate_keypair()?;

    let proposal: calimero_context_config::types::Signed<Proposal> = proxy_helper.create_proposal(
        &alice_sk,
        &config_helper.config_contract.as_account(),
        vec![],
    )?;

    let res: ProposalWithApprovals = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    let res2 = proxy_helper
        .approve_proposal(&relayer_account, &bob_sk, &res.proposal_id)
        .await?
        .into_result();

    let error = res2.expect_err("Expected an error from the contract");
    assert!(error.to_string().contains("Not a context member"));

    let view_proposal: ProposalWithApprovals = proxy_helper
        .view_proposal_confirmations(&relayer_account, &res.proposal_id)
        .await?
        .json()?;
    assert_eq!(view_proposal.num_approvals, 1);

    Ok(())
}

#[tokio::test]
async fn test_execute_proposal() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (config_helper, proxy_helper, relayer_account, context_sk, alice_sk) =
        setup_test(&worker).await?;

    let counter_helper = CounterContracttHelper::deploy_and_initialize(&worker).await?;

    let proposal = proxy_helper.create_proposal(
        &alice_sk,
        &counter_helper.counter_contract.as_account(),
        vec![ProposalAction::FunctionCall {
            method_name: "increment".to_string(),
            args: Base64VecU8::from(vec![]),
            deposit: NearToken::from_near(0),
            gas: Gas::from_gas(1_000_000_000_000),
        }],
    )?;

    // 4. Create and approve the proposal by Alice (or other required accounts)
    let res: ProposalWithApprovals = proxy_helper
        .create_and_approve_proposal(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    // Check initial approvals
    assert_eq!(res.num_approvals, 1);

    // 5. Add more approvals if necessary to trigger the execution threshold
    // Assuming the threshold is 2 approvals, we add another approver
    let bob_sk = common::generate_keypair()?;
    let charlie_sk = common::generate_keypair()?;
    let _res = config_helper
        .add_members(
            &relayer_account,
            &alice_sk,
            &[bob_sk.clone(), charlie_sk.clone()],
            &context_sk,
        )
        .await?
        .into_result()?;

    // Approve the proposal with Bob's signature
    let res2: ProposalWithApprovals = proxy_helper
        .approve_proposal(&relayer_account, &bob_sk, &res.proposal_id)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res2.num_approvals, 2, "Proposal should have 2 approvals");

    let counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(
        counter_value, 0,
        "Counter should be zero before proposal execution"
    );

    let _res3: ProposalWithApprovals = proxy_helper
        .approve_proposal(&relayer_account, &charlie_sk, &res.proposal_id)
        .await?
        .into_result()?
        .json()?;

    let counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(
        counter_value, 1,
        "Counter should be incremented by the proposal execution"
    );

    Ok(())
}
