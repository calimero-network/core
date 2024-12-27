use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

use eyre::{bail, Result as EyreResult};
use near_workspaces::network::Sandbox;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract, Worker};
use rand::seq::SliceRandom;
use serde_json::from_slice;
use tokio::fs::{read, read_dir};
use tokio::time::sleep;

use crate::config::Config;
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::output::OutputWriter;
use crate::steps::{TestScenario, TestStep};
use crate::TestEnvironment;

pub struct TestContext<'a> {
    pub inviter: String,
    pub invitees: Vec<String>,
    pub meroctl: &'a Meroctl,
    pub application_id: Option<String>,
    pub context_id: Option<String>,
    pub inviter_public_key: Option<String>,
    pub invitees_public_keys: HashMap<String, String>,
    pub output_writer: OutputWriter,
}

pub trait Test {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()>;
}

impl<'a> TestContext<'a> {
    pub fn new(
        inviter: String,
        invitees: Vec<String>,
        meroctl: &'a Meroctl,
        output_writer: OutputWriter,
    ) -> Self {
        Self {
            inviter,
            invitees,
            meroctl,
            application_id: None,
            context_id: None,
            inviter_public_key: None,
            invitees_public_keys: HashMap::new(),
            output_writer,
        }
    }
}

pub struct Driver {
    environment: TestEnvironment,
    config: Config,
    meroctl: Meroctl,
    merods: HashMap<String, Merod>,
    near: Option<NearSandboxEnvironment>,
}

pub struct NearSandboxEnvironment {
    pub worker: Worker<Sandbox>,
    pub root_account: Account,
    pub contract: Contract,
}

impl Driver {
    pub fn new(environment: TestEnvironment, config: Config) -> Self {
        let meroctl = Meroctl::new(&environment);
        Self {
            environment,
            config,
            meroctl,
            merods: HashMap::new(),
            near: None,
        }
    }

    pub async fn run(&mut self) -> EyreResult<()> {
        self.environment.init().await?;

        self.init_near_environment().await?;

        let result = {
            self.boot_merods().await?;
            self.run_scenarios().await
        };

        self.stop_merods().await;

        if let Err(e) = &result {
            self.environment
                .output_writer
                .write_str("Error occurred during test run:");
            self.environment.output_writer.write_string(e.to_string());
        }

        result
    }

    async fn init_near_environment(&mut self) -> EyreResult<()> {
        let worker = near_workspaces::sandbox().await?;

        let wasm = read(&self.config.near.context_config_contract).await?;
        let context_config_contract = worker.dev_deploy(&wasm).await?;

        let proxy_lib_contract = read(&self.config.near.proxy_lib_contract).await?;
        drop(
            context_config_contract
                .call("set_proxy_code")
                .args(proxy_lib_contract)
                .max_gas()
                .transact()
                .await?
                .into_result()?,
        );

        let root_account = worker.root_account()?;

        self.near = Some(NearSandboxEnvironment {
            worker,
            root_account,
            contract: context_config_contract,
        });

        Ok(())
    }

    async fn boot_merods(&mut self) -> EyreResult<()> {
        self.environment
            .output_writer
            .write_header("Starting merod nodes", 2);

        for i in 0..self.config.network.node_count {
            let node_name = format!("node{}", i + 1);
            if !self.merods.contains_key(&node_name) {
                let mut args = vec![
                    format!(
                        "discovery.rendezvous.namespace=\"calimero/e2e-tests/{}\"",
                        self.environment.test_id
                    ),
                    format!("sync.interval_ms={}", 10_000),
                    format!("sync.timeout_ms={}", 10_000),
                ];

                if let Some(ref near) = self.near {
                    let near_account = near
                        .root_account
                        .create_subaccount(&node_name)
                        .initial_balance(NearToken::from_near(30))
                        .transact()
                        .await?
                        .into_result()?;
                    let near_secret_key = near_account.secret_key();

                    args.extend(vec![
                        format!(
                            "context.config.new.contract_id=\"{}\"",
                            near.contract.as_account().id()
                        ),
                        format!("context.config.signer.use=\"{}\"", "self"),
                        format!(
                            "context.config.signer.self.near.testnet.rpc_url=\"{}\"",
                            near.worker.rpc_addr()
                        ),
                        format!(
                            "context.config.signer.self.near.testnet.account_id=\"{}\"",
                            near_account.id()
                        ),
                        format!(
                            "context.config.signer.self.near.testnet.public_key=\"{}\"",
                            near_secret_key.public_key()
                        ),
                        format!(
                            "context.config.signer.self.near.testnet.secret_key=\"{}\"",
                            near_secret_key
                        ),
                    ]);
                }

                let mut config_args = vec![];
                config_args.extend(args.iter().map(|arg| &**arg));

                let merod = Merod::new(node_name.clone(), &self.environment);

                let swarm_host = match env::var(&self.config.network.swarm_host_env) {
                    Ok(host) => host,
                    Err(_) => "0.0.0.0".to_string(),
                };

                merod
                    .init(
                        &swarm_host,
                        self.config.network.start_swarm_port + i,
                        self.config.network.start_server_port + i,
                        &config_args,
                    )
                    .await?;

                merod.run().await?;

                drop(self.merods.insert(node_name, merod));
            }
        }

        // TODO: Implement health check?
        sleep(Duration::from_secs(10)).await;

        Ok(())
    }

    async fn stop_merods(&mut self) {
        for (_, merod) in self.merods.iter() {
            if let Err(err) = merod.stop().await {
                eprintln!("Error stopping merod: {:?}", err);
            }
        }

        self.merods.clear();
    }

    async fn run_scenarios(&self) -> EyreResult<()> {
        let scenarios_dir = self.environment.input_dir.join("scenarios");
        let mut entries = read_dir(scenarios_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let test_file_path = path.join("test.json");
                if test_file_path.exists() {
                    self.run_scenario(test_file_path).await?;
                }
            }
        }

        Ok(())
    }

    async fn run_scenario(&self, file_path: PathBuf) -> EyreResult<()> {
        self.environment
            .output_writer
            .write_header("Running scenario", 2);

        let scenario: TestScenario = from_slice(&read(&file_path).await?)?;

        self.environment
            .output_writer
            .write_string(format!("Source file: {:?}", file_path));
        self.environment
            .output_writer
            .write_string(format!("Steps count: {}", scenario.steps.len()));

        let (inviter, invitees) = match self.pick_inviter_node() {
            Some((inviter, invitees)) => (inviter, invitees),
            None => bail!("Not enough nodes to run the test"),
        };

        self.environment
            .output_writer
            .write_string(format!("Picked inviter: {}", inviter));
        self.environment
            .output_writer
            .write_string(format!("Picked invitees: {:?}", invitees));

        let mut ctx = TestContext::new(
            inviter,
            invitees,
            &self.meroctl,
            self.environment.output_writer,
        );

        for step in scenario.steps.iter() {
            self.environment
                .output_writer
                .write_header("Running test step", 3);
            self.environment.output_writer.write_str("Step spec:");
            self.environment.output_writer.write_json(&step)?;

            match step {
                TestStep::ApplicationInstall(step) => step.run_assert(&mut ctx).await?,
                TestStep::ContextCreate(step) => step.run_assert(&mut ctx).await?,
                TestStep::ContextInviteJoin(step) => step.run_assert(&mut ctx).await?,
                TestStep::JsonRpcCall(step) => step.run_assert(&mut ctx).await?,
            };
        }

        Ok(())
    }

    fn pick_inviter_node(&self) -> Option<(String, Vec<String>)> {
        let mut node_names: Vec<String> = self.merods.keys().cloned().collect();
        if node_names.len() < 1 {
            None
        } else {
            let mut rng = rand::thread_rng();
            node_names.shuffle(&mut rng);
            let picked_node = node_names.remove(0);
            Some((picked_node, node_names))
        }
    }
}
