use near_sdk::json_types::U128;
use near_sdk::NearToken;
use serde_json::json;

#[tokio::test]
async fn test_score_board_contract() -> Result<(), Box<dyn std::error::Error>> {
    let sandbox = near_workspaces::sandbox().await?;
    let wasm = tokio::fs::read("res/leaderboard.wasm").await?;

    let contract = sandbox.dev_deploy(&wasm).await?;

    let alice_account = sandbox.dev_create_account().await?;
    let bob_account = sandbox.dev_create_account().await?;

    let alice_outcome = alice_account
        .call(contract.id(), "add_score")
        .args_json(json!({"app_name": "test_app", "account_id": alice_account.id(), "score": "10"}))
        .deposit(NearToken::from_near(0))
        .transact()
        .await?;

    assert!(alice_outcome.is_success());

    let score: Option<U128> = contract
        .view("get_score")
        .args_json(json!({"app_name": "test_app", "account_id": alice_account.id()}))
        .await?
        .json()?;

    assert_eq!(score, Some(U128(10)));

    let score: Option<U128> = contract
        .view("get_score")
        .args_json(json!({"app_name": "test_app", "account_id": bob_account.id()}))
        .await?
        .json()?;

    assert_eq!(score, None);

    let alice_outcome = alice_account
        .call(contract.id(), "add_score")
        .args_json(
            json!({"app_name": "test_app_2", "account_id": alice_account.id(), "score": "100"}),
        )
        .deposit(NearToken::from_near(0))
        .transact()
        .await?;

    assert!(alice_outcome.is_success());

    let score: Option<U128> = contract
        .view("get_score")
        .args_json(json!({"app_name": "test_app", "account_id": alice_account.id()}))
        .await?
        .json()?;
    assert_eq!(score, Some(U128(10)));

    let score: Option<U128> = contract
        .view("get_score")
        .args_json(json!({"app_name": "test_app_2", "account_id": alice_account.id()}))
        .await?
        .json()?;
    assert_eq!(score, Some(U128(100)));

    Ok(())
}
