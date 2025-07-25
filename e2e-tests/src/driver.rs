use core::fmt::Write;
use std::collections::btree_map::{BTreeMap, Entry as BTreeMapEntry};
use std::collections::hash_map::HashMap;
use std::path::PathBuf;

use calimero_sandbox::config::{DevnetConfig, ProtocolConfigs};
use calimero_sandbox::protocol::ProtocolSandboxEnvironment;
use calimero_sandbox::Devnet;
use camino::Utf8Path;
use eyre::{bail, Result as EyreResult};
use rand::seq::IteratorRandom;
use serde::{Deserialize, Serialize};
use serde_json::from_slice;
use tokio::fs::{read, read_dir, read_to_string, write};

use crate::config::{Config, ProtocolSandboxConfig};
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::output::OutputWriter;
use crate::steps::TestScenario;
use crate::{Protocol, TestEnvironment};

pub struct TestContext<'a> {
    pub inviter: String,
    pub invitees: Vec<String>,
    pub meroctl: &'a Meroctl,
    pub application_id: Option<String>,
    pub context_id: Option<String>,
    pub inviter_public_key: Option<String>,
    pub invitees_public_keys: HashMap<String, String>,
    pub protocol_name: &'a Protocol,
    pub protocol: &'a ProtocolSandboxEnvironment,
    pub output_writer: OutputWriter,
    pub context_alias: Option<String>,
    pub proposal_id: Option<String>,
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
        protocol_name: &'a Protocol,
        proposal_id: Option<String>,
        protocol: &'a ProtocolSandboxEnvironment,
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
            context_alias: None,
            proposal_id,
            protocol,
        }
    }
}

pub struct Driver {
    environment: TestEnvironment,
    config: Config,
}

pub struct MeroWithDevnet {
    ctl: Meroctl,
    ds: HashMap<String, Merod>,
    devnet: Devnet,
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

        let mut report = TestRunReport::new();

        let protocols_dir = self.environment.input_dir.join("protocols");

        for protocol_name in &self.environment.protocols {
            let protocol_path = protocols_dir.join(protocol_name.as_str());

            if !protocol_path.is_dir() {
                self.environment.output_writer.write_str(&format!(
                    "No directory for protocol: {}",
                    protocol_name.as_str()
                ));
                continue;
            }

            let mero = self.setup_mero(protocol_name).await?;

            // Get protocol environment from devnet
            let protocol_env = mero
                .devnet
                .get_protocol_environment(protocol_name.as_str())?;

            let Some((inviter, invitees)) = self.pick_inviter_node(&mero.ds) else {
                bail!(
                    "Not enough nodes to run test for protocol {}",
                    protocol_name.as_str()
                )
            };

            self.environment
                .output_writer
                .write_str(&format!("Picked inviter: {inviter}"));
            self.environment
                .output_writer
                .write_str(&format!("Picked invitees: {invitees:?}"));

            let mut applications = read_dir(&protocol_path).await?;
            while let Some(app) = applications.next_entry().await? {
                if !app.file_type().await?.is_file() {
                    continue;
                }
                let mut ctx = TestContext::new(
                    inviter.clone(),
                    invitees.clone(),
                    &mero.ctl,
                    self.environment.output_writer,
                    protocol_name,
                    None,
                    protocol_env,
                );
                let test_file_path = app.path();

                let Some(app_name) = test_file_path.file_stem().and_then(|s| s.to_str()) else {
                    bail!("No application name found");
                };

                if !test_file_path.is_file() {
                    continue;
                }
                let test_content = read(&test_file_path).await?;
                let scenario: TestScenario = from_slice(&test_content)?;

                self.environment
                    .output_writer
                    .write_header(&format!("Running protocol {}", protocol_env.name()), 1);

                report = self
                    .run_scenarios(&mut ctx, report, app_name, scenario, &test_file_path)
                    .await?;

                self.environment
                    .output_writer
                    .write_header(&format!("Finished protocol {}", protocol_env.name()), 1);
            }

            self.stop_merods(&mero.ds).await;
        }

        if let Err(e) = report.result() {
            self.environment
                .output_writer
                .write_str("Error occurred during test run:");
            self.environment.output_writer.write_str(&e.to_string());
        }

        report
            .store_to_file(
                &self.environment.output_dir,
                &self.environment.output_writer,
            )
            .await?;

        report.result()
    }

    async fn setup_mero(&self, protocol_name: &Protocol) -> eyre::Result<MeroWithDevnet> {
        self.environment
            .output_writer
            .write_header("Starting merod nodes", 2);

        let config = self.load_sandbox_config(protocol_name).await?;
        let mut devnet = Devnet::new(config)?;

        devnet.start().await?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let mut merods = HashMap::new();
        for (name, node) in &devnet.nodes {
            let merod = Merod::new(self.environment.merod_binary.clone());
            let node_dir = self.environment.nodes_dir.join(name);
            if !node_dir.exists() {
                tokio::fs::create_dir_all(&node_dir).await?;
            }

            // Get protocol-specific args
            let protocol_env = devnet.get_protocol_environment(protocol_name.as_str())?;
            let node_args = protocol_env.node_args(name).await?;

            merod
                .start(&self.environment.nodes_dir, name, node_args)
                .await?;
            merods.insert(name.clone(), merod);
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        Ok(MeroWithDevnet {
            ctl: Meroctl::new(
                self.environment.nodes_dir.clone(),
                self.environment.meroctl_binary.clone(),
                self.environment.output_writer,
            ),
            ds: merods,
            devnet,
        })
    }

    async fn load_sandbox_config(&self, protocol: &Protocol) -> EyreResult<DevnetConfig> {
        let config_path = self.environment.input_dir.join("config.json");
        let config_content = read_to_string(config_path).await?;
        let config: Config = serde_json::from_str(&config_content)?;

        Ok(DevnetConfig {
            node_count: config.network.node_count,
            protocols: vec![protocol.as_str().to_string()],
            protocol_configs: ProtocolConfigs {
                near: config
                    .protocol_sandboxes
                    .iter()
                    .find_map(|c| match c {
                        ProtocolSandboxConfig::Near(near) => Some(near.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| eyre::eyre!("Near config not found"))?,
                icp: config
                    .protocol_sandboxes
                    .iter()
                    .find_map(|c| match c {
                        ProtocolSandboxConfig::Icp(icp) => Some(icp.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| eyre::eyre!("ICP config not found"))?,
                stellar: config
                    .protocol_sandboxes
                    .iter()
                    .find_map(|c| match c {
                        ProtocolSandboxConfig::Stellar(stellar) => Some(stellar.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| eyre::eyre!("Stellar config not found"))?,
                ethereum: config
                    .protocol_sandboxes
                    .iter()
                    .find_map(|c| match c {
                        ProtocolSandboxConfig::Ethereum(ethereum) => Some(ethereum.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| eyre::eyre!("Ethereum config not found"))?,
            },
            swarm_host: config.network.swarm_host.to_string(),
            start_swarm_port: config.network.start_swarm_port,
            server_host: config.network.server_host.to_string(),
            start_server_port: config.network.start_server_port,
            home_dir: self.environment.nodes_dir.clone(),
            node_name: "devnet".into(),
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
        ctx: &mut TestContext<'_>,
        mut report: TestRunReport,
        app_name: &str,
        scenario: TestScenario,
        file_path: &PathBuf,
    ) -> EyreResult<TestRunReport> {
        let scenario_report = self
            .run_scenario(ctx, app_name, scenario, file_path)
            .await?;

        drop(
            report
                .scenario_matrix
                .entry(ctx.protocol_name.as_str().to_owned())
                .or_default()
                .insert(app_name.to_owned(), scenario_report),
        );

        Ok(report)
    }

    async fn run_scenario(
        &self,
        ctx: &mut TestContext<'_>,
        app_name: &str,
        scenario: TestScenario,
        file_path: &PathBuf,
    ) -> EyreResult<TestScenarioReport> {
        self.environment
            .output_writer
            .write_header("Running scenario", 2);

        self.environment
            .output_writer
            .write_str(&format!("Source file: {file_path:?}"));
        self.environment
            .output_writer
            .write_str(&format!("Steps count: {}", scenario.steps.len()));

        let mut report = TestScenarioReport::new(app_name.to_owned());

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

            let result = step.run_assert(ctx).await;

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

#[derive(Serialize, Deserialize)]
pub struct TestRunReport {
    scenario_matrix: BTreeMap<String, BTreeMap<String, TestScenarioReport>>,
}

impl TestRunReport {
    fn new() -> Self {
        Self {
            scenario_matrix: BTreeMap::default(),
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

    pub async fn store_to_file(
        &self,
        output_dir: &Utf8Path,
        output_writer: &OutputWriter,
    ) -> EyreResult<()> {
        let markdown = self.to_markdown()?;
        let json = serde_json::to_string_pretty(&self)?;

        let report_file = output_dir.join("report.md");
        write(&report_file, markdown).await?;

        output_writer.write_str(&format!("Report file (.md): {report_file:?}"));

        let report_file = output_dir.join("report.json");
        write(&report_file, json).await?;

        output_writer.write_str(&format!("Report file (.json): {report_file:?}"));

        Ok(())
    }

    pub async fn from_dir(dir: &Utf8Path) -> EyreResult<Self> {
        let file = dir.join("report.json");
        let content = read(&file).await?;
        let report = from_slice(&content)?;
        Ok(report)
    }

    pub async fn merge(&mut self, other: Self) {
        for (scenario, other_protocols) in other.scenario_matrix {
            let protocols = self.scenario_matrix.entry(scenario).or_default();

            for (protocol, other_report) in other_protocols {
                let entry = protocols.entry(protocol);

                match entry {
                    BTreeMapEntry::Occupied(mut entry) => {
                        let report = entry.get_mut();

                        for step in other_report.steps {
                            if report.steps.iter().all(|s| s.step_name != step.step_name) {
                                report.steps.push(step);
                            }
                        }
                    }
                    BTreeMapEntry::Vacant(entry) => {
                        entry.insert(other_report);
                    }
                }
            }
        }
    }

    fn to_markdown(&self) -> EyreResult<String> {
        let mut markdown = String::new();
        writeln!(&mut markdown, "## E2E tests report")?;

        for (protocol, applications) in &self.scenario_matrix {
            writeln!(&mut markdown, "### Protocol: {protocol}")?;

            for (app_name, report) in applications {
                let mut step_names = vec![];
                for step in &report.steps {
                    if !step_names.contains(&step.step_name) {
                        step_names.push(step.step_name.clone());
                    }
                }

                // Write table header
                write!(&mut markdown, "| Application/Step |")?;
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
                write!(&mut markdown, "| {app_name} |")?;
                // Results row
                for step_name in &step_names {
                    let result = report.steps.iter().find_map(|step| {
                        (&step.step_name == step_name).then_some(step.result.as_ref())
                    });
                    let result = match result {
                        None => "-",
                        Some(None) => ":fast_forward:",
                        Some(Some(Ok(_))) => ":white_check_mark:",
                        Some(Some(Err(_))) => ":x:",
                    };
                    write!(&mut markdown, " {result} |")?;
                }
                writeln!(&mut markdown, "\n")?;
            }
            writeln!(&mut markdown)?;
        }

        Ok(markdown)
    }
}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
struct TestStepReport {
    step_name: String,
    #[serde(default, with = "serde_eyre", skip_serializing_if = "Option::is_none")]
    result: Option<EyreResult<()>>,
}

mod serde_eyre {
    use eyre::bail;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Outcome {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }

    pub fn serialize<S>(result: &Option<eyre::Result<()>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        result
            .as_ref()
            .map(|result| Outcome {
                error: result.as_ref().err().map(|err| err.to_string()),
            })
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<eyre::Result<()>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let outcome = Outcome::deserialize(deserializer)?;

        Ok(Some(
            outcome.error.map_or_else(|| Ok(()), |error| bail!(error)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::net::IpAddr;

    use calimero_sandbox::port_binding::PortBinding;

    #[tokio::test]
    async fn test_ports() -> eyre::Result<()> {
        let env_hosts = env::var("TEST_HOSTS").ok();

        dbg!(&env_hosts);

        let mut env_hosts = env_hosts
            .iter()
            .flat_map(|hosts| hosts.split(','))
            .map(|host| host.parse::<IpAddr>())
            .into_iter()
            .peekable();

        let default = env_hosts
            .peek()
            .map_or_else(|| Some(Ok([0, 0, 0, 0].into())), |_| None)
            .into_iter();

        let port = 2800;

        for host in default.chain(env_hosts) {
            let host = host?;

            dbg!(&host, port);

            test_port(host, port).await?;
        }

        Ok(())
    }

    async fn test_port(host: IpAddr, start_port: u16) -> eyre::Result<()> {
        let mut port = start_port;

        let bind1 = PortBinding::next_available(host, &mut port).await?;

        assert_eq!(port, bind1.port() + 1);

        let bind2 = PortBinding::next_available(host, &mut port).await?;

        assert_eq!(port, bind2.port() + 1);

        let port1 = bind1.into_socket_addr().port();
        let port2 = bind2.into_socket_addr().port();

        assert!(port1 < port2);

        let bind1 = PortBinding::next_available(host, &mut { port1 }).await?;
        let bind2 = PortBinding::next_available(host, &mut { port2 }).await?;

        assert_eq!(bind1.port(), port1);
        assert_eq!(bind2.port(), port2);

        Ok(())
    }
}
