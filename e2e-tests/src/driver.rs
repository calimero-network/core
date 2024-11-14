use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use eyre::{bail, Result as EyreResult};
use rand::seq::SliceRandom;
use serde_json::from_slice;
use tokio::fs::{read, read_dir};
use tokio::time::sleep;

use crate::config::Config;
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::steps::{TestScenario, TestStep};
use crate::TestEnvironment;

pub struct TestContext<'a> {
    pub inviter: String,
    pub invitees: Vec<String>,
    pub meroctl: &'a Meroctl,
    pub context_id: Option<String>,
    pub inviter_public_key: Option<String>,
    pub invitees_public_keys: HashMap<String, String>,
}

pub trait Test {
    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()>;
}

impl<'a> TestContext<'a> {
    pub fn new(inviter: String, invitees: Vec<String>, meroctl: &'a Meroctl) -> Self {
        Self {
            inviter,
            invitees,
            meroctl,
            context_id: None,
            inviter_public_key: None,
            invitees_public_keys: HashMap::new(),
        }
    }
}

pub struct Driver {
    environment: TestEnvironment,
    config: Config,
    meroctl: Meroctl,
    merods: RefCell<HashMap<String, Merod>>,
}

impl Driver {
    pub fn new(environment: TestEnvironment, config: Config) -> Self {
        let meroctl = Meroctl::new(&environment);
        Self {
            environment,
            config,
            meroctl,
            merods: RefCell::new(HashMap::new()),
        }
    }

    pub async fn run(&self) -> EyreResult<()> {
        self.environment.init().await?;

        let result = self.run_tests().await;

        self.stop_nodes().await;

        result
    }

    async fn run_tests(&self) -> EyreResult<()> {
        self.boot_nodes().await?;

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
        println!("================= Setting up scenario and context ==================");
        let scenario: TestScenario = from_slice(&read(&file_path).await?)?;

        println!(
            "Loaded test scenario from file: {:?}\n{:?}",
            file_path, scenario
        );

        let (inviter, invitees) = match self.pick_inviter_node() {
            Some((inviter, invitees)) => (inviter, invitees),
            None => bail!("Not enough nodes to run the test"),
        };

        println!("Picked inviter: {}", inviter);
        println!("Picked invitees: {:?}", invitees);

        let mut ctx = TestContext::new(inviter, invitees, &self.meroctl);

        println!("====================================================================");

        for step in scenario.steps.iter() {
            println!("======================== Starting step =============================");
            println!("Step: {:?}", step);
            match step {
                TestStep::ContextCreate(step) => step.run_assert(&mut ctx).await?,
                TestStep::ContextInviteJoin(step) => step.run_assert(&mut ctx).await?,
                TestStep::JsonRpcCall(step) => step.run_assert(&mut ctx).await?,
            };
            println!("====================================================================");
        }

        Ok(())
    }

    fn pick_inviter_node(&self) -> Option<(String, Vec<String>)> {
        let merods = self.merods.borrow();
        let mut node_names: Vec<String> = merods.keys().cloned().collect();
        if node_names.len() < 1 {
            None
        } else {
            let mut rng = rand::thread_rng();
            node_names.shuffle(&mut rng);
            let picked_node = node_names.remove(0);
            Some((picked_node, node_names))
        }
    }

    async fn boot_nodes(&self) -> EyreResult<()> {
        println!("========================= Starting nodes ===========================");

        for i in 0..self.config.network.node_count {
            let node_name = format!("node{}", i + 1);
            let mut merods = self.merods.borrow_mut();
            if !merods.contains_key(&node_name) {
                let merod = Merod::new(node_name.clone(), &self.environment);
                let args: Vec<&str> = self.config.merod.args.iter().map(|s| s.as_str()).collect();

                merod
                    .init(
                        self.config.network.start_swarm_port + i,
                        self.config.network.start_server_port + i,
                        &args,
                    )
                    .await?;

                merod.run().await?;

                drop(merods.insert(node_name, merod));
            }
        }

        // TODO: Implement health check?
        sleep(Duration::from_secs(20)).await;

        println!("====================================================================");

        Ok(())
    }

    async fn stop_nodes(&self) {
        let mut merods = self.merods.borrow_mut();

        for (_, merod) in merods.iter_mut() {
            if let Err(err) = merod.stop().await {
                eprintln!("Error stopping merod: {:?}", err);
            }
        }

        merods.clear();
    }
}
