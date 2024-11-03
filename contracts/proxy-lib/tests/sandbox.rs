use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::{ProposalAction, ProposalWithApprovals};
use common::config_helper::ConfigContractHelper;
use common::counter_helper::CounterContractHelper;
use common::create_account_with_balance;
use common::proxy_lib_helper::ProxyContractHelper;
use ed25519_dalek::SigningKey;
use eyre::Result;
use near_sdk::NearToken;
use near_workspaces::network::Sandbox;
use near_workspaces::{Account, Worker};
use serde_json::json;

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
    let (_config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    let proposal = proxy_helper.create_proposal_request(&alice_sk, &vec![])?;

    let res: Option<ProposalWithApprovals> = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    match res {
        Some(proposal) => {
            assert_eq!(proposal.proposal_id, 0, "Expected proposal_id to be 0");
            assert_eq!(proposal.num_approvals, 1, "Expected 1 approval");
        }
        None => panic!("Expected to create a proposal, but got None"),
    }

    Ok(())
}

#[tokio::test]
async fn test_create_proposal_by_non_member() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (_config_helper, proxy_helper, relayer_account, _context_sk, _alice_sk) =
        setup_test(&worker).await?;

    // Bob is not a member of the context
    let bob_sk: SigningKey = common::generate_keypair()?;

    let proposal = proxy_helper.create_proposal_request(&bob_sk, &vec![])?;

    let res = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
        .await?
        .into_result();

    let error = res.expect_err("Expected an error from the contract");
    assert!(error.to_string().contains("Is not a member"));

    let view_proposal: Option<ProposalWithApprovals> = proxy_helper
        .view_proposal_confirmations(&relayer_account, &0)
        .await?
        .json()?;

    match view_proposal {
        Some(proposal) => panic!("Expected to not create a proposal, but got {:?}", proposal),
        None => Ok(()),
    }
}

#[tokio::test]
async fn test_create_multiple_proposals() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let (_config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    let proposal = proxy_helper.create_proposal_request(&alice_sk, &vec![])?;

    let _res = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
        .await?
        .into_result();

    let res: ProposalWithApprovals = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
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

    let proposal = proxy_helper.create_proposal_request(&alice_sk, &vec![])?;

    let res: ProposalWithApprovals = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
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

    let (_config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    // Bob is not a member of the context
    let bob_sk: SigningKey = common::generate_keypair()?;

    let proposal = proxy_helper.create_proposal_request(&alice_sk, &vec![])?;

    let res: ProposalWithApprovals = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    let res2 = proxy_helper
        .approve_proposal(&relayer_account, &bob_sk, &res.proposal_id)
        .await?
        .into_result();

    let error = res2.expect_err("Expected an error from the contract");
    assert!(error.to_string().contains("Is not a member"));

    let view_proposal: ProposalWithApprovals = proxy_helper
        .view_proposal_confirmations(&relayer_account, &res.proposal_id)
        .await?
        .json()?;
    assert_eq!(view_proposal.num_approvals, 1);

    Ok(())
}

async fn setup_action_test(
    worker: &Worker<Sandbox>,
) -> Result<(ProxyContractHelper, Account, Vec<SigningKey>)> {
    let (config_helper, proxy_helper, relayer_account, context_sk, alice_sk) =
        setup_test(&worker).await?;

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

    let members = vec![alice_sk, bob_sk, charlie_sk];
    Ok((proxy_helper, relayer_account, members))
}

async fn create_and_approve_proposal(
    proxy_helper: &ProxyContractHelper,
    relayer_account: &Account,
    actions: &Vec<ProposalAction>,
    members: Vec<SigningKey>,
) -> Result<()> {
    let proposal = proxy_helper.create_proposal_request(&members[0], actions)?;

    let res: ProposalWithApprovals = proxy_helper
        .proxy_mutate(&relayer_account, &proposal)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res.num_approvals, 1);

    let res: ProposalWithApprovals = proxy_helper
        .approve_proposal(&relayer_account, &members[1], &res.proposal_id)
        .await?
        .into_result()?
        .json()?;

    assert_eq!(res.num_approvals, 2, "Proposal should have 2 approvals");

    let res: Option<ProposalWithApprovals> = proxy_helper
        .approve_proposal(&relayer_account, &members[2], &res.proposal_id)
        .await?
        .into_result()?
        .json()?;

    assert!(
        res.is_none(),
        "Proposal should be removed after the execution"
    );

    Ok(())
}

#[tokio::test]
async fn test_execute_proposal() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let counter_helper = CounterContractHelper::deploy_and_initialize(&worker).await?;

    let counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(
        counter_value, 0,
        "Counter should be zero before proposal execution"
    );

    let actions = vec![ProposalAction::ExternalFunctionCall {
        receiver_id: counter_helper.counter_contract.id().to_string(),
        method_name: "increment".to_string(),
        args: serde_json::to_string(&Vec::<u8>::new())?,
        deposit: 0,
        gas: 1_000_000_000_000,
    }];
    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(
        counter_value, 1,
        "Counter should be incremented by the proposal execution"
    );

    Ok(())
}

#[tokio::test]
async fn test_action_change_active_proposals_limit() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let default_active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(default_active_proposals_limit, 10);

    let actions = vec![ProposalAction::SetActiveProposalsLimit {
        active_proposals_limit: 6,
    }];
    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let new_active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(new_active_proposals_limit, 6);

    Ok(())
}

#[tokio::test]
async fn test_action_change_number_of_approvals() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let default_new_num_approvals: u32 = proxy_helper.view_num_approvals(&relayer_account).await?;
    assert_eq!(default_new_num_approvals, 3);

    let actions = vec![ProposalAction::SetNumApprovals { num_approvals: 2 }];
    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let new_num_approvals: u32 = proxy_helper.view_num_approvals(&relayer_account).await?;
    assert_eq!(new_num_approvals, 2);

    Ok(())
}

#[tokio::test]
async fn test_mutate_storage_value() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let key_data = b"example_key".to_vec().into_boxed_slice();
    let value_data = b"example_value".to_vec().into_boxed_slice();

    let default_storage_value: Option<Box<[u8]>> = proxy_helper
        .view_context_value(&relayer_account, key_data.clone())
        .await?;
    assert!(default_storage_value.is_none());

    let actions = vec![ProposalAction::SetContextValue {
        key: key_data.clone(),
        value: value_data.clone(),
    }];
    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let storage_value: Box<[u8]> = proxy_helper
        .view_context_value(&relayer_account, key_data.clone())
        .await?
        .expect("Expected some value, but got None");
    assert_eq!(
        storage_value, value_data,
        "The value did not match the expected data"
    );

    Ok(())
}

#[tokio::test]
async fn test_transfer() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let _res = &worker
        .root_account()?
        .transfer_near(
            proxy_helper.proxy_contract.id(),
            near_workspaces::types::NearToken::from_near(5),
        )
        .await?;

    let recipient = create_account_with_balance(&worker, "new_account", 0).await?;

    let recipient_balance = recipient.view_account().await?.balance;
    assert_eq!(
        NearToken::from_near(0).as_near(),
        recipient_balance.as_near()
    );

    let actions = vec![ProposalAction::Transfer {
        receiver_id: recipient.id().to_string(),
        amount: 5,
    }];
    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let recipient_balance = recipient.view_account().await?.balance;
    assert_eq!(
        NearToken::from_near(5).as_near(),
        recipient_balance.as_near()
    );

    Ok(())
}

#[tokio::test]
async fn test_combined_proposals() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let counter_helper = CounterContractHelper::deploy_and_initialize(&worker).await?;

    let initial_counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(initial_counter_value, 0, "Counter should start at zero");

    let initial_active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(
        initial_active_proposals_limit, 10,
        "Default proposals limit should be 10"
    );

    let actions = vec![
        ProposalAction::ExternalFunctionCall {
            receiver_id: counter_helper.counter_contract.id().to_string(),
            method_name: "increment".to_string(),
            args: serde_json::to_string(&Vec::<u8>::new())?,
            deposit: 0,
            gas: 1_000_000_000_000,
        },
        ProposalAction::SetActiveProposalsLimit {
            active_proposals_limit: 5,
        },
    ];

    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let updated_counter_value: u32 = counter_helper.get_value().await?;
    assert_eq!(
        updated_counter_value, 1,
        "Counter should be incremented by the proposal execution"
    );

    let updated_active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(
        updated_active_proposals_limit, 5,
        "Active proposals limit should be updated to 5"
    );

    Ok(())
}

#[tokio::test]
async fn test_combined_proposal_actions_with_promise_failure() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let (proxy_helper, relayer_account, members) = setup_action_test(&worker).await?;

    let counter_helper = CounterContractHelper::deploy_and_initialize(&worker).await?;

    let initial_active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(
        initial_active_proposals_limit, 10,
        "Default proposals limit should be 10"
    );

    let actions = vec![
        ProposalAction::ExternalFunctionCall {
            receiver_id: counter_helper.counter_contract.id().to_string(),
            method_name: "non_existent_method".to_string(), // This method does not exist
            args: serde_json::to_string(&Vec::<u8>::new())?,
            deposit: 0,
            gas: 1_000_000_000_000,
        },
        ProposalAction::SetActiveProposalsLimit {
            active_proposals_limit: 5,
        },
    ];

    let _res =
        create_and_approve_proposal(&proxy_helper, &relayer_account, &actions, members).await;

    let active_proposals_limit: u32 = proxy_helper
        .view_active_proposals_limit(&relayer_account)
        .await?;
    assert_eq!(
        active_proposals_limit, 10,
        "Active proposals limit should remain unchanged due to the failed promise"
    );

    Ok(())
}

#[tokio::test]
async fn test_view_proposals() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;

    let (_config_helper, proxy_helper, relayer_account, _context_sk, alice_sk) =
        setup_test(&worker).await?;

    let proposal1_actions = vec![ProposalAction::SetActiveProposalsLimit {
        active_proposals_limit: 5,
    }];
    let proposal1 = proxy_helper.create_proposal_request(&alice_sk, &proposal1_actions)?;
    let proposal2_actions = vec![ProposalAction::SetNumApprovals { num_approvals: 2 }];
    let proposal2 = proxy_helper.create_proposal_request(&alice_sk, &proposal2_actions)?;
    let proposal3_actions = vec![ProposalAction::SetContextValue {
        key: b"example_key".to_vec().into_boxed_slice(),
        value: b"example_value".to_vec().into_boxed_slice(),
    }];
    let proposal3 = proxy_helper.create_proposal_request(&alice_sk, &proposal3_actions)?;

    let _ = proxy_helper
        .proxy_mutate(&relayer_account, &proposal1)
        .await?;
    let _ = proxy_helper
        .proxy_mutate(&relayer_account, &proposal2)
        .await?;
    let _ = proxy_helper
        .proxy_mutate(&relayer_account, &proposal3)
        .await?;

    let proposals = proxy_helper.view_proposals(&relayer_account, 0, 3).await?;

    assert_eq!(proposals.len(), 3, "Expected to retrieve 3 proposals");

    assert_eq!(proposals[0].0, 0, "Expected first proposal to have id 0");
    assert_eq!(proposals[1].0, 1, "Expected second proposal to have id 1");
    assert_eq!(proposals[2].0, 2, "Expected third proposal to have id 2");

    assert_eq!(
        &proposals[0].1.actions[0], &proposal1_actions[0],
        "First proposal actions should match proposal 1"
    );
    assert_eq!(
        &proposals[1].1.actions[0], &proposal2_actions[0],
        "Second proposal actions should match proposal 2"
    );
    assert_eq!(
        &proposals[2].1.actions[0], &proposal3_actions[0],
        "Third proposal actions should match proposal 3"
    );

    // Retrieve proposals with offset 1 and length 2
    let proposals = proxy_helper.view_proposals(&relayer_account, 1, 2).await?;

    assert_eq!(
        proposals.len(),
        2,
        "Expected to retrieve 2 proposals starting from offset 1"
    );

    assert_eq!(
        proposals[0].0, 1,
        "Expected the first returned proposal to have id 1"
    );
    assert_eq!(
        proposals[1].0, 2,
        "Expected the second returned proposal to have id 2"
    );

    assert_eq!(
        &proposals[0].1.actions[0], &proposal2_actions[0],
        "First proposal actions should match proposal 2"
    );
    assert_eq!(
        &proposals[1].1.actions[0], &proposal3_actions[0],
        "Second proposal actions should match proposal 3"
    );

    // Verify retrieval with a length of 1
    let single_proposal = proxy_helper.view_proposals(&relayer_account, 2, 1).await?;

    assert_eq!(
        single_proposal.len(),
        1,
        "Expected to retrieve 1 proposal starting from offset 3"
    );
    assert_eq!(
        single_proposal[0].0, 2,
        "Expected the proposal to have id 2"
    );

    assert_eq!(
        &proposals[1].1.actions[0], &proposal3_actions[0],
        "first proposal actions should match proposal 3"
    );

    Ok(())
}
