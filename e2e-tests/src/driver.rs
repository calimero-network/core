use core::fmt::Write;
use core::time::Duration;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::PathBuf;

use camino::Utf8PathBuf;
use eyre::{bail, OptionExt, Result as EyreResult};
use rand::seq::IteratorRandom;
use serde_json::from_slice;
use tokio::fs::{read, read_dir, write};
use tokio::time::sleep;

use crate::config::{Config, ProtocolSandboxConfig};
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::output::OutputWriter;
use crate::protocol::icp::IcpSandboxEnvironment;
use crate::protocol::near::NearSandboxEnvironment;
use crate::protocol::stellar::StellarSandboxEnvironment;
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
    pub protocol_name: &'a str,
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
        protocol_name: &'a str,
    ) -> Self {
        Self {
            inviter,
            invitees,
            meroctl,
            application_id: None,
            context_id: None,
            protocol_name,
            inviter_public_key: None,
            invitees_public_keys: HashMap::new(),
            output_writer,
        }
    }
}

pub struct Driver {
    environment: TestEnvironment,
    config: Config,
}

pub struct Mero {
    ctl: Meroctl,
    ds: HashMap<String, Merod>,
}

impl Driver {
    pub const fn new(environment: TestEnvironment, config: Config) -> Self {
        Self {
            environment,
            config,
        }
    }

    pub async fn run(&self) -> EyreResult<()> {
        self.environment.init().await?;

        let mut sandbox_environments: Vec<ProtocolSandboxEnvironment> = Vec::default();
        for protocol_sandbox in &self.config.protocol_sandboxes {
            let protocol_name = match protocol_sandbox {
                ProtocolSandboxConfig::Stellar(_) => "stellar",
                ProtocolSandboxConfig::Near(_) => "near",
                ProtocolSandboxConfig::Icp(_) => "icp",
            };

            if !self
                .environment
                .protocols
                .iter()
                .any(|p| p.to_lowercase() == protocol_name)
            {
                continue;
            }

            match protocol_sandbox {
                ProtocolSandboxConfig::Stellar(config) => {
                    let stellar = StellarSandboxEnvironment::init(config.clone())?;
                    sandbox_environments.push(ProtocolSandboxEnvironment::Stellar(stellar));
                }
                ProtocolSandboxConfig::Near(config) => {
                    let near = NearSandboxEnvironment::init(config.clone()).await?;
                    sandbox_environments.push(ProtocolSandboxEnvironment::Near(near));
                }
                ProtocolSandboxConfig::Icp(config) => {
                    let icp = IcpSandboxEnvironment::init(config.clone())?;
                    sandbox_environments.push(ProtocolSandboxEnvironment::Icp(icp));
                }
            }
        }

        let mut report = TestRunReport::new();
        let mero = self.setup_mero(&sandbox_environments).await?;
        for sandbox in &sandbox_environments {
            self.environment
                .output_writer
                .write_header(&format!("Running protocol {}", sandbox.name()), 1);

            report = self.run_scenarios(&mero, report, sandbox.name()).await?;

            self.environment
                .output_writer
                .write_header(&format!("Finished protocol {}", sandbox.name()), 1);
        }

        self.stop_merods(&mero.ds).await;

        if let Err(e) = report.result() {
            self.environment
                .output_writer
                .write_str("Error occurred during test run:");
            self.environment.output_writer.write_str(&e.to_string());
        }

        let report_file = report.store_to_file(&self.environment.output_dir).await?;

        self.environment
            .output_writer
            .write_str(&format!("Report file: {report_file:?}"));

        report.result()
    }

    async fn setup_mero(
        &self,
        sandbox_environments: &Vec<ProtocolSandboxEnvironment>,
    ) -> EyreResult<Mero> {
        self.environment
            .output_writer
            .write_header("Starting merod nodes", 2);

        let mut merods = HashMap::new();

        for i in 0..self.config.network.node_count {
            let node_name = format!("node{}", i + 1);
            if let Entry::Vacant(e) = merods.entry(node_name.clone()) {
                let config_args = [format!(
                    "discovery.rendezvous.namespace=\"calimero/e2e-tests/{}\"",
                    self.environment.test_id
                )];

                let mut node_args = vec![];
                for sandbox in sandbox_environments {
                    node_args = sandbox.node_args(&node_name).await?;
                }

                let config_args = config_args.iter().chain(node_args.iter());

                let merod = Merod::new(
                    node_name,
                    self.environment.nodes_dir.clone(),
                    &self.environment.logs_dir,
                    self.environment.merod_binary.clone(),
                    self.environment.output_writer,
                );

                merod
                    .init(
                        &self.config.network.swarm_host,
                        self.config.network.start_swarm_port + i,
                        self.config.network.start_server_port + i,
                        config_args.map(String::as_str),
                    )
                    .await?;

                merod.run().await?;

                let _ = e.insert(merod);
            }
        }

        // TODO: Implement health check?
        sleep(Duration::from_secs(10)).await;

        Ok(Mero {
            ctl: Meroctl::new(
                self.environment.nodes_dir.clone(),
                self.environment.meroctl_binary.clone(),
                self.environment.output_writer,
            ),
            ds: merods,
        })
    }

    async fn stop_merods(&self, merods: &HashMap<String, Merod>) {
        for (_, merod) in merods {
            if let Err(err) = merod.stop().await {
                eprintln!("Error stopping merod: {err:?}");
            }
        }
    }

    async fn run_scenarios(
        &self,
        mero: &Mero,
        mut report: TestRunReport,
        protocol_name: &str,
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
                            mero,
                            path.file_name()
                                .ok_or_eyre("failed to get scenario file name")?
                                .to_str()
                                .ok_or_eyre("failed to convert scenario file name")?,
                            test_file_path,
                            protocol_name,
                        )
                        .await?;

                    drop(
                        report
                            .scenario_matrix
                            .entry(scenario_report.scenario_name.clone())
                            .or_default()
                            .insert(protocol_name.to_owned(), scenario_report),
                    );
                }
            }
        }

        Ok(report)
    }

    async fn run_scenario(
        &self,
        mero: &Mero,
        scenarion_name: &str,
        file_path: PathBuf,
        protocol_name: &str,
    ) -> EyreResult<TestScenarioReport> {
        self.environment
            .output_writer
            .write_header("Running scenario", 2);

        let scenario: TestScenario = from_slice(&read(&file_path).await?)?;

        self.environment
            .output_writer
            .write_str(&format!("Source file: {file_path:?}"));
        self.environment
            .output_writer
            .write_str(&format!("Steps count: {}", scenario.steps.len()));

        let Some((inviter, invitees)) = self.pick_inviter_node(&mero.ds) else {
            bail!("Not enough nodes to run the test")
        };

        self.environment
            .output_writer
            .write_str(&format!("Picked inviter: {inviter}"));
        self.environment
            .output_writer
            .write_str(&format!("Picked invitees: {invitees:?}"));

        let mut ctx = TestContext::new(
            inviter,
            invitees,
            &mero.ctl,
            self.environment.output_writer,
            protocol_name,
        );

        let mut report = TestScenarioReport::new(scenarion_name.to_owned());

        let mut scenario_failed = false;
        for (i, step) in scenario.steps.iter().enumerate() {
            if scenario_failed {
                report.steps.push(TestStepReport {
                    step_name: format!("{}. {}", i, step.display_name()),
                    result: None,
                });
                continue;
            }

            self.environment
                .output_writer
                .write_header("Running test step", 3);
            self.environment.output_writer.write_str("Step spec:");
            self.environment.output_writer.write_json(&step)?;

            let result = step.run_assert(&mut ctx).await;

            if result.is_err() {
                scenario_failed = true;
                self.environment
                    .output_writer
                    .write_str(&format!("Error: {result:?}"));
            }

            report.steps.push(TestStepReport {
                step_name: format!("{}. {}", i, step.display_name()),
                result: Some(result),
            });
        }

        Ok(report)
    }

    fn pick_inviter_node(&self, merods: &HashMap<String, Merod>) -> Option<(String, Vec<String>)> {
        let mut rng = rand::thread_rng();
        let mut node_names: Vec<String> = merods.keys().cloned().collect();
        let picked_node = node_names.iter().choose(&mut rng);
        if picked_node.is_none() {
            None
        } else {
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
            scenario_matrix: HashMap::default(),
        }
    }

    fn result(&self) -> EyreResult<()> {
        let mut errors = vec![];

        for (_, scenarios) in &self.scenario_matrix {
            for (_, scenario) in scenarios {
                for step in &scenario.steps {
                    if let Some(Err(e)) = &step.result {
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

    async fn store_to_file(&self, folder: &Utf8PathBuf) -> EyreResult<Utf8PathBuf> {
        let markdown = self.to_markdown()?;
        let report_file = folder.join("report.md");
        write(&report_file, markdown).await?;
        Ok(report_file)
    }

    fn to_markdown(&self) -> EyreResult<String> {
        let mut markdown = String::new();

        writeln!(&mut markdown, "## E2E tests report")?;

        for (scenario, protocols) in &self.scenario_matrix {
            writeln!(&mut markdown, "### Scenario: {scenario}")?;

            // Collecting all step names
            let mut step_names = vec![];
            for report in protocols.values() {
                for step in &report.steps {
                    if !step_names.contains(&step.step_name) {
                        step_names.push(step.step_name.clone());
                    }
                }
            }

            // Write table header
            write!(&mut markdown, "| Protocol/Step |")?;
            for step_name in &step_names {
                write!(&mut markdown, " {step_name} |")?;
            }
            writeln!(&mut markdown)?;

            // Write table header separator
            write!(&mut markdown, "| :--- |")?;
            for _ in &step_names {
                write!(&mut markdown, " :---: |")?;
            }
            writeln!(&mut markdown)?;

            // Write table rows
            for (protocol, report) in protocols {
                write!(&mut markdown, "| {protocol} |")?;
                for step_name in &step_names {
                    let result = report
                        .steps
                        .iter()
                        .find(|step| &step.step_name == step_name)
                        .map_or(":bug:", |step| {
                            step.result
                                .as_ref()
                                .map_or(":fast_forward:", |result| match result {
                                    Ok(()) => ":white_check_mark:",
                                    Err(_) => ":x:",
                                })
                        });
                    write!(&mut markdown, " {result} |")?;
                }
                writeln!(&mut markdown)?;
            }
            writeln!(&mut markdown)?;
        }

        Ok(markdown)
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
            steps: Vec::default(),
        }
    }
}

struct TestStepReport {
    step_name: String,
    result: Option<EyreResult<()>>,
}
