//! Local NEAR sandbox management for dev mode.
//!
//! Replicates the approach from merobox's `SandboxManager`:
//! download near-sandbox binary, init, start, deploy Calimero contracts.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use eyre::{bail, Context, Result};

const NEAR_SANDBOX_VERSION: &str = "2.9.0";
const NEAR_SANDBOX_AWS_BASE: &str =
    "https://s3-us-west-1.amazonaws.com/build.nearprotocol.com/nearcore";
const SANDBOX_RPC_PORT: u16 = 3030;

const CONTRACTS_REPO: &str = "calimero-network/contracts";
const CONTRACTS_VERSION: &str = "0.6.0";
const CONFIG_WASM: &str = "calimero_context_config_near.wasm";
const PROXY_WASM: &str = "calimero_context_proxy_near.wasm";

pub struct DevSandbox {
    process: Option<Child>,
    home_dir: PathBuf,
    pub rpc_url: String,
    pub contract_id: String,
}

impl DevSandbox {
    pub fn rpc_port() -> u16 {
        SANDBOX_RPC_PORT
    }

    /// Check if a sandbox is already running on the default port.
    pub async fn is_running() -> bool {
        reqwest::get(format!("http://localhost:{SANDBOX_RPC_PORT}/status"))
            .await
            .is_ok()
    }

    /// Start a local NEAR sandbox, deploy Calimero contracts.
    pub async fn start() -> Result<Self> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?
            .join(".calimero")
            .join("sandbox");

        std::fs::create_dir_all(&home_dir)?;

        let binary_path = ensure_sandbox_binary(&home_dir).await?;

        cleanup_stale_sandbox();

        let data_dir = home_dir.join("data");
        if data_dir.exists() {
            std::fs::remove_dir_all(&data_dir)?;
        }

        eprintln!("  Initializing NEAR sandbox...");
        let data_str = data_dir.display().to_string();
        let bin = binary_path.clone();
        tokio::task::spawn_blocking(move || {
            let status = Command::new(&bin)
                .args(["--home", &data_str, "init"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !status.success() {
                bail!("near-sandbox init failed");
            }
            Ok::<_, eyre::Report>(())
        })
        .await??;

        eprintln!("  Starting NEAR sandbox on port {SANDBOX_RPC_PORT}...");
        let data_str = data_dir.display().to_string();
        let process = Command::new(&binary_path)
            .args([
                "--home",
                &data_str,
                "run",
                "--rpc-addr",
                &format!("0.0.0.0:{SANDBOX_RPC_PORT}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let rpc_url = format!("http://localhost:{SANDBOX_RPC_PORT}");

        wait_for_rpc(&rpc_url).await?;
        eprintln!("  NEAR sandbox running");

        let key_path = data_dir.join("validator_key.json");
        let key_data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&key_path)?)?;
        let root_account_id = key_data["account_id"].as_str().unwrap().to_owned();
        let root_secret_key = key_data["secret_key"].as_str().unwrap().to_owned();

        let contracts_dir = ensure_contracts().await?;
        let contract_id =
            deploy_calimero_contracts(&rpc_url, &root_account_id, &root_secret_key, &contracts_dir)
                .await?;

        Ok(Self {
            process: Some(process),
            home_dir,
            rpc_url,
            contract_id,
        })
    }

    pub async fn create_node_account(
        &self,
        node_name: &str,
        root_account_id: &str,
        root_secret_key: &str,
    ) -> Result<NearAccountCredentials> {
        let account_id = format!("{node_name}.test.near");
        create_near_account(
            &self.rpc_url,
            root_account_id,
            root_secret_key,
            &account_id,
            50,
        )
        .await
    }

    pub fn root_credentials(&self) -> Result<(String, String)> {
        let key_path = self.home_dir.join("data").join("validator_key.json");
        let key_data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&key_path)?)?;
        let account_id = key_data["account_id"].as_str().unwrap().to_owned();
        let secret_key = key_data["secret_key"].as_str().unwrap().to_owned();
        Ok((account_id, secret_key))
    }
}

impl Drop for DevSandbox {
    fn drop(&mut self) {
        if let Some(mut proc) = self.process.take() {
            let _ = proc.kill();
            let _ = proc.wait();
            eprintln!("  NEAR sandbox stopped");
        }
    }
}

pub struct NearAccountCredentials {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

// --- Binary management ---

fn platform_name() -> Result<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => Ok("Darwin-arm64"),
        ("linux", "x86_64") => Ok("Linux-x86_64"),
        ("linux", "aarch64") => Ok("Linux-aarch64"),
        _ => bail!("Unsupported platform: {os} {arch}"),
    }
}

async fn ensure_sandbox_binary(home_dir: &Path) -> Result<PathBuf> {
    let platform = platform_name()?;
    let binary_path = home_dir.join(platform).join("near-sandbox");

    if binary_path.exists() {
        return Ok(binary_path);
    }

    let url =
        format!("{NEAR_SANDBOX_AWS_BASE}/{platform}/{NEAR_SANDBOX_VERSION}/near-sandbox.tar.gz");
    eprintln!("  Downloading near-sandbox from {url}...");

    let response = reqwest::get(&url)
        .await
        .context("Failed to download near-sandbox")?
        .error_for_status()
        .context("near-sandbox download returned error status")?;

    let bytes = response.bytes().await?;

    let hd = home_dir.to_owned();
    let bp = binary_path.clone();
    tokio::task::spawn_blocking(move || {
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        std::fs::create_dir_all(&hd)?;
        archive.unpack(&hd)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bp, std::fs::Permissions::from_mode(0o755))?;
        }

        Ok::<_, eyre::Report>(())
    })
    .await??;

    eprintln!("  near-sandbox installed");
    Ok(binary_path)
}

fn cleanup_stale_sandbox() {
    let _ = Command::new("pkill")
        .args(["-9", "near-sandbox"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    std::thread::sleep(Duration::from_millis(500));
}

async fn wait_for_rpc(rpc_url: &str) -> Result<()> {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let url = format!("{rpc_url}/status");

    for _ in 0..50 {
        if reqwest::get(&url).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    bail!("NEAR sandbox failed to start within 10 seconds")
}

// --- Contract management ---

async fn ensure_contracts() -> Result<PathBuf> {
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?
        .join(".calimero")
        .join("contracts")
        .join(CONTRACTS_VERSION)
        .join("near");

    let config_wasm = cache_dir.join(CONFIG_WASM);
    let proxy_wasm = cache_dir.join(PROXY_WASM);

    if config_wasm.exists() && proxy_wasm.exists() {
        return Ok(cache_dir);
    }

    std::fs::create_dir_all(&cache_dir)?;

    let api_url =
        format!("https://api.github.com/repos/{CONTRACTS_REPO}/releases/tags/{CONTRACTS_VERSION}");
    eprintln!("  Fetching Calimero contracts {CONTRACTS_VERSION}...");

    let client = reqwest::Client::new();
    let release: serde_json::Value = client
        .get(&api_url)
        .header("User-Agent", "meroctl")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| eyre::eyre!("No assets in release"))?;
    let download_url = assets
        .iter()
        .find(|a| a["name"].as_str() == Some("near.tar.gz"))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| eyre::eyre!("near.tar.gz not found in release assets"))?
        .to_owned();

    eprintln!("  Downloading contracts...");
    let response = client
        .get(&download_url)
        .header("User-Agent", "meroctl")
        .send()
        .await?
        .error_for_status()?;

    let bytes = response.bytes().await?;

    let cd = cache_dir.clone();
    let cw = config_wasm.clone();
    let pw = proxy_wasm.clone();
    tokio::task::spawn_blocking(move || {
        let gz = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);

        let temp_dir = cd.parent().unwrap().join("_extract_tmp");
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir)?;
        }
        std::fs::create_dir_all(&temp_dir)?;
        archive.unpack(&temp_dir)?;

        move_contracts_to_cache(&temp_dir, &cd)?;
        let _ = std::fs::remove_dir_all(&temp_dir);

        if !cw.exists() || !pw.exists() {
            bail!("Contract files not found after extraction");
        }

        Ok::<_, eyre::Report>(())
    })
    .await??;

    eprintln!("  Calimero contracts ready");
    Ok(cache_dir)
}

fn move_contracts_to_cache(extract_dir: &Path, cache_dir: &Path) -> Result<()> {
    for entry in walkdir(extract_dir)? {
        if let Some(name) = entry.file_name().and_then(|n| n.to_str()) {
            if name == CONFIG_WASM || name == PROXY_WASM {
                let dest = cache_dir.join(name);
                std::fs::copy(&entry, &dest)?;
            }
        }
    }
    Ok(())
}

fn walkdir(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                results.extend(walkdir(&path)?);
            } else {
                results.push(path);
            }
        }
    }
    Ok(results)
}

// --- NEAR transaction helpers ---

use near_crypto::{InMemorySigner, SecretKey, Signer};
use near_jsonrpc_client::methods::query::RpcQueryRequest;
use near_jsonrpc_client::methods::send_tx::RpcSendTransactionRequest;
use near_jsonrpc_client::JsonRpcClient;
use near_primitives::action::{
    Action, AddKeyAction, CreateAccountAction, DeployContractAction, FunctionCallAction,
    TransferAction,
};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Transaction, TransactionV0};
use near_primitives::types::{AccountId, BlockReference};
use near_primitives::views::{AccessKeyView, QueryRequest, TxExecutionStatus};

async fn get_nonce_and_block(
    client: &JsonRpcClient,
    account_id: &AccountId,
    public_key: &near_crypto::PublicKey,
) -> Result<(u64, CryptoHash)> {
    let response = client
        .call(RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: account_id.clone(),
                public_key: public_key.clone(),
            },
        })
        .await?;

    if let near_jsonrpc_primitives::types::query::QueryResponseKind::AccessKey(AccessKeyView {
        nonce,
        ..
    }) = response.kind
    {
        Ok((nonce, response.block_hash))
    } else {
        bail!("Unexpected response when fetching access key")
    }
}

async fn sign_and_send(
    client: &JsonRpcClient,
    signer_id: &AccountId,
    secret_key: &SecretKey,
    receiver_id: &AccountId,
    actions: Vec<Action>,
) -> Result<()> {
    let public_key = secret_key.public_key();
    let (nonce, block_hash) = get_nonce_and_block(client, signer_id, &public_key).await?;

    let transaction = Transaction::V0(TransactionV0 {
        signer_id: signer_id.clone(),
        public_key,
        nonce: nonce.saturating_add(1),
        receiver_id: receiver_id.clone(),
        block_hash,
        actions,
    });

    let signer = Signer::InMemory(InMemorySigner::from_secret_key(
        signer_id.clone(),
        secret_key.clone(),
    ));

    let _response = client
        .call(RpcSendTransactionRequest {
            signed_transaction: transaction.sign(&signer),
            wait_until: TxExecutionStatus::Final,
        })
        .await
        .context("Transaction failed")?;

    Ok(())
}

async fn create_near_account(
    rpc_url: &str,
    root_account_id: &str,
    root_secret_key: &str,
    new_account_id: &str,
    initial_balance_near: u128,
) -> Result<NearAccountCredentials> {
    let client = JsonRpcClient::connect(rpc_url);
    let root_id: AccountId = root_account_id.parse()?;
    let root_sk: SecretKey = root_secret_key.parse()?;
    let new_id: AccountId = new_account_id.parse()?;

    let new_sk = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let new_pk = new_sk.public_key();

    let amount_yocto = initial_balance_near * 10u128.pow(24);

    sign_and_send(
        &client,
        &root_id,
        &root_sk,
        &new_id,
        vec![
            Action::CreateAccount(CreateAccountAction {}),
            Action::Transfer(TransferAction {
                deposit: amount_yocto,
            }),
            Action::AddKey(Box::new(AddKeyAction {
                public_key: new_pk.clone(),
                access_key: near_primitives::account::AccessKey::full_access(),
            })),
        ],
    )
    .await?;

    Ok(NearAccountCredentials {
        account_id: new_account_id.to_owned(),
        public_key: new_pk.to_string(),
        secret_key: new_sk.to_string(),
    })
}

async fn deploy_calimero_contracts(
    rpc_url: &str,
    root_account_id: &str,
    root_secret_key: &str,
    contracts_dir: &Path,
) -> Result<String> {
    let contract_account = "calimero.test.near";

    eprintln!("  Creating {contract_account}...");
    let creds = create_near_account(
        rpc_url,
        root_account_id,
        root_secret_key,
        contract_account,
        100,
    )
    .await?;

    let client = JsonRpcClient::connect(rpc_url);
    let contract_id: AccountId = contract_account.parse()?;
    let contract_sk: SecretKey = creds.secret_key.parse()?;

    eprintln!("  Deploying context-config contract...");
    let config_wasm = std::fs::read(contracts_dir.join(CONFIG_WASM))?;
    sign_and_send(
        &client,
        &contract_id,
        &contract_sk,
        &contract_id,
        vec![Action::DeployContract(DeployContractAction {
            code: config_wasm,
        })],
    )
    .await?;

    eprintln!("  Setting proxy code...");
    let proxy_wasm = std::fs::read(contracts_dir.join(PROXY_WASM))?;
    sign_and_send(
        &client,
        &contract_id,
        &contract_sk,
        &contract_id,
        vec![Action::FunctionCall(Box::new(FunctionCallAction {
            method_name: "set_proxy_code".to_owned(),
            args: proxy_wasm,
            gas: 300_000_000_000_000,
            deposit: 0,
        }))],
    )
    .await?;

    eprintln!("  Contracts deployed to {contract_account}");
    Ok(contract_account.to_owned())
}

/// Patch a node's config.toml to use the local NEAR sandbox.
pub fn patch_node_config(
    config_path: &Path,
    rpc_url: &str,
    contract_id: &str,
    account_id: &str,
    public_key: &str,
    secret_key: &str,
) -> Result<()> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    let mut config: toml::Value = content.parse().context("Failed to parse config.toml")?;

    let table = config.as_table_mut().unwrap();

    fn ensure_table<'a>(
        t: &'a mut toml::map::Map<String, toml::Value>,
        key: &str,
    ) -> &'a mut toml::map::Map<String, toml::Value> {
        if !t.contains_key(key) {
            t.insert(key.to_owned(), toml::Value::Table(toml::map::Map::new()));
        }
        t.get_mut(key).unwrap().as_table_mut().unwrap()
    }

    let context = ensure_table(table, "context");
    let cfg = ensure_table(context, "config");
    let near = ensure_table(cfg, "near");
    near.insert(
        "network".to_owned(),
        toml::Value::String("local".to_owned()),
    );
    near.insert(
        "contract_id".to_owned(),
        toml::Value::String(contract_id.to_owned()),
    );
    near.insert("signer".to_owned(), toml::Value::String("self".to_owned()));

    let signer = ensure_table(cfg, "signer");
    let self_signer = ensure_table(signer, "self");
    let near_signer = ensure_table(self_signer, "near");
    let local = ensure_table(near_signer, "local");
    local.insert(
        "rpc_url".to_owned(),
        toml::Value::String(rpc_url.to_owned()),
    );
    local.insert(
        "account_id".to_owned(),
        toml::Value::String(account_id.to_owned()),
    );
    local.insert(
        "public_key".to_owned(),
        toml::Value::String(public_key.to_owned()),
    );
    local.insert(
        "secret_key".to_owned(),
        toml::Value::String(secret_key.to_owned()),
    );

    std::fs::write(config_path, toml::to_string_pretty(&config)?)?;

    Ok(())
}
