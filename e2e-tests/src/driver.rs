use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

use eyre::{bail, OptionExt, Result as EyreResult};
use rand::seq::SliceRandom;
use serde_json::from_slice;
use tokio::fs::{read, read_dir};
use tokio::time::sleep;

use crate::config::{Config, ProtocolSandboxConfig};
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::output::OutputWriter;
use crate::protocol::icp::IcpSandboxEnvironment;
use crate::protocol::near::NearSandboxEnvironment;
use crate::protocol::ProtocolSandboxEnvironment;
use crate::steps::TestScenario;
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
    fn display_name(&self) -> String;
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
}

impl Driver {
    pub fn new(environment: TestEnvironment, config: Config) -> Self {
        let meroctl = Meroctl::new(&environment);
        Self {
            environment,
            config,
            meroctl,
            merods: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> EyreResult<()> {
        self.environment.init().await?;

        let mut sandbox_environments: Vec<ProtocolSandboxEnvironment> = Default::default();
        for protocol_sandbox in self.config.protocol_sandboxes.iter() {
            match protocol_sandbox {
                ProtocolSandboxConfig::Near(config) => {
                    let near = NearSandboxEnvironment::init(config.clone()).await?;
                    sandbox_environments.push(ProtocolSandboxEnvironment::Near(near));
                }
                ProtocolSandboxConfig::Icp(config) => {
                    let icp = IcpSandboxEnvironment::init(config.clone()).await?;
                    sandbox_environments.push(ProtocolSandboxEnvironment::Icp(icp));
                }
            }
        }

        let mut report = TestRunReport::new();
        for sandbox in sandbox_environments.iter() {
            self.boot_merods(sandbox).await?;
            report = self.run_scenarios(report, sandbox.name()).await?;
            self.stop_merods().await;
        }

        if let Err(e) = report.result() {
            self.environment
                .output_writer
                .write_str("Error occurred during test run:");
            self.environment.output_writer.write_string(e.to_string());
        }

        println!("{}", report.to_markdown());
        report.result()
    }

    async fn boot_merods(&mut self, sandbox: &ProtocolSandboxEnvironment) -> EyreResult<()> {
        self.environment
            .output_writer
            .write_header("Starting merod nodes", 2);

        for i in 0..self.config.network.node_count {
            let node_name = format!("node{}", i + 1);
            if !self.merods.contains_key(&node_name) {
                let mut args = vec![format!(
                    "discovery.rendezvous.namespace=\"calimero/e2e-tests/{}\"",
                    self.environment.test_id
                )];

                args.extend(sandbox.node_args(&node_name).await?);

                let mut config_args = vec![];
                config_args.extend(args.iter().map(|arg| &**arg));

                let merod = Merod::new(node_name.clone(), &self.environment);

                let swarm_host = match env::var(&self.config.network.swarm_host_env) {
                    Ok(host) => host,
                    Err(_) => "0.0.0.0".to_owned(),
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

    async fn run_scenarios(
        &self,
        mut report: TestRunReport,
        protocol_name: String,
    ) -> EyreResult<TestRunReport> {
        let scenarios_dir = self.environment.input_dir.join("scenarios");
        let mut entries = read_dir(scenarios_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let test_file_path = path.join("test.json");
                if test_file_path.exists() {
                    let scenario_report = self
                        .run_scenario(
                            path.file_name()
                                .ok_or_eyre("failed")?
                                .to_str()
                                .ok_or_eyre("failed")?,
                            test_file_path,
                        )
                        .await?;

                    drop(
                        report
                            .scenario_matrix
                            .entry(scenario_report.scenario_name.clone())
                            .or_insert_with(HashMap::new)
                            .insert(protocol_name.clone(), scenario_report),
                    );
                }
            }
        }

        Ok(report)
    }

    async fn run_scenario(
        &self,
        scenarion_name: &str,
        file_path: PathBuf,
    ) -> EyreResult<TestScenarioReport> {
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

        let mut report = TestScenarioReport::new(scenarion_name.to_owned());

        for step in scenario.steps.iter() {
            self.environment
                .output_writer
                .write_header("Running test step", 3);
            self.environment.output_writer.write_str("Step spec:");
            self.environment.output_writer.write_json(&step)?;

            let result = step.run_assert(&mut ctx).await;
            report.steps.push(TestStepReport {
                step_name: step.display_name(),
                result,
            });
        }

        Ok(report)
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

struct TestRunReport {
    scenario_matrix: HashMap<String, HashMap<String, TestScenarioReport>>,
}

impl TestRunReport {
    fn new() -> Self {
        Self {
            scenario_matrix: Default::default(),
        }
    }

    fn result(&self) -> EyreResult<()> {
        let mut errors = vec![];

        for (_, scenarios) in &self.scenario_matrix {
            for (_, scenario) in scenarios {
                for step in &scenario.steps {
                    if let Err(e) = &step.result {
                        errors.push(e.to_string());
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("Errors occurred during test run: {:?}", errors)
        }
    }

    fn to_markdown(&self) -> String {
        let mut markdown = String::new();

        for (scenario, protocols) in &self.scenario_matrix {
            markdown.push_str(&format!("## Scenario: {}\n", scenario));
            markdown.push_str("| Protocol/Step |");

            // Collecting all step names
            let mut step_names = vec![];
            for report in protocols.values() {
                for step in &report.steps {
                    if !step_names.contains(&step.step_name) {
                        step_names.push(step.step_name.clone());
                    }
                }
            }

            // Adding step names to the first row of the table
            for step_name in &step_names {
                markdown.push_str(&format!(" {} |", step_name));
            }
            markdown.push_str("\n| :--- |");
            for _ in &step_names {
                markdown.push_str(" :---: |");
            }
            markdown.push_str("\n");

            // Adding protocol rows
            for (protocol, report) in protocols {
                markdown.push_str(&format!("| {} |", protocol));
                for step_name in &step_names {
                    let result = report
                        .steps
                        .iter()
                        .find(|step| &step.step_name == step_name)
                        .map_or("N/A", |step| {
                            if step.result.is_ok() {
                                "Success"
                            } else {
                                "Failure"
                            }
                        });
                    markdown.push_str(&format!(" {} |", result));
                }
                markdown.push_str("\n");
            }
            markdown.push_str("\n");
        }
        markdown
    }
}

struct TestScenarioReport {
    scenario_name: String,
    steps: Vec<TestStepReport>,
}

impl TestScenarioReport {
    fn new(scenario_name: String) -> Self {
        Self {
            scenario_name,
            steps: Default::default(),
        }
    }
}

struct TestStepReport {
    step_name: String,
    result: EyreResult<()>,
}
