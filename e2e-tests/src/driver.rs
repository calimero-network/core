use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use eyre::{bail, Result as EyreResult};
use rand::seq::SliceRandom;
use tokio::fs::{read, read_dir};
use tokio::time::sleep;

use crate::config::Config;
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::steps::{TestScenario, TestStep};
use crate::TestEnvironment;

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

        result?;

        Ok(())
    }

    async fn run_tests(&self) -> EyreResult<()> {
        self.boot_nodes().await?;

        use serde_json::from_slice;

        let scenarios_dir = self.environment.input_dir.join("scenarios");
        let mut entries = read_dir(scenarios_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let test_file_path = path.join("test.json");
                if test_file_path.exists() {
                    let scenario = from_slice(&read(&test_file_path).await?)?;

                    println!(
                        "Loaded test scenario from file: {:?}\n{:?}",
                        test_file_path, scenario
                    );

                    self.run_scenario(scenario).await?;
                }
            }
        }

        Ok(())
    }

    async fn run_scenario(&self, scenario: TestScenario) -> EyreResult<()> {
        let (inviter_node, invitee_node) = match self.pick_two_nodes() {
            Some((inviter_node, invitee_node)) => (inviter_node, invitee_node),
            None => bail!("Not enough nodes to run the test"),
        };

        let ctx = TestContext::new(inviter_node, invitee_node, &self.meroctl);

        for step in scenario.steps.iter() {
            println!("Running step: {:?}", step);
            match step {
                TestStep::ContextCreate(step) => step.run_assert(&ctx).await?,
                TestStep::ContextInviteJoin(step) => step.run_assert(&ctx).await?,
                TestStep::JsonRpcExecute(step) => step.run_assert(&ctx).await?,
            };
        }

        Ok(())
    }

    fn pick_two_nodes(&self) -> Option<(String, String)> {
        let merods = self.merods.borrow();
        let mut node_names: Vec<String> = merods.keys().cloned().collect();
        if node_names.len() < 2 {
            None
        } else {
            let mut rng = rand::thread_rng();
            node_names.shuffle(&mut rng);
            Some((node_names[0].clone(), node_names[1].clone()))
        }
    }

    async fn boot_nodes(&self) -> EyreResult<()> {
        for i in 0..self.config.network_layout.node_count {
            let node_name = format!("node{}", i + 1);
            let mut merods = self.merods.borrow_mut();
            if !merods.contains_key(&node_name) {
                let merod = Merod::new(node_name.clone(), &self.environment);

                merod
                    .init(
                        self.config.network_layout.start_swarm_port + i,
                        self.config.network_layout.start_server_port + i,
                    )
                    .await?;

                merod.run().await?;

                drop(merods.insert(node_name, merod));
            }
        }

        // TODO: Implement health check?
        sleep(Duration::from_secs(20)).await;

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

pub struct TestContext<'a> {
    pub inviter_node: String,
    pub invitee_node: String,
    pub meroctl: &'a Meroctl,
    context_id: RefCell<Option<String>>,
    inviter_public_key: RefCell<Option<String>>,
}

pub trait Test {
    async fn run_assert(&self, ctx: &TestContext<'_>) -> EyreResult<()>;
}

impl<'a> TestContext<'a> {
    pub fn new(inviter_node: String, invitee_node: String, meroctl: &'a Meroctl) -> Self {
        Self {
            inviter_node,
            invitee_node,
            meroctl,
            context_id: RefCell::new(None),
            inviter_public_key: RefCell::new(None),
        }
    }

    pub fn set_context_id(&self, context_id: String) {
        *self.context_id.borrow_mut() = Some(context_id);
    }

    pub fn get_context_id(&self) -> Option<String> {
        self.context_id.borrow().clone()
    }

    pub fn set_inviter_public_key(&self, inviter_public_key: String) {
        *self.inviter_public_key.borrow_mut() = Some(inviter_public_key);
    }

    pub fn get_inviter_public_key(&self) -> Option<String> {
        self.inviter_public_key.borrow().clone()
    }
}
