use core::fmt::Write;
use std::collections::btree_map::{BTreeMap, Entry as BTreeMapEntry};
use std::collections::hash_map::{Entry as HashMapEntry, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use camino::Utf8Path;
use eyre::{bail, Result as EyreResult};
use rand::seq::IteratorRandom;
use serde::{Deserialize, Serialize};
use serde_json::from_slice;
use tokio::fs::{read, read_dir, write};
use tokio::net::{TcpListener, TcpSocket};
use tokio::time::{sleep, Duration};
use tokio::try_join;

use crate::config::{Config, ProtocolSandboxConfig};
use crate::meroctl::Meroctl;
use crate::merod::Merod;
use crate::output::OutputWriter;
use crate::protocol::ethereum::EthereumSandboxEnvironment;
use crate::protocol::icp::IcpSandboxEnvironment;
use crate::protocol::near::NearSandboxEnvironment;
use crate::protocol::stellar::StellarSandboxEnvironment;
use crate::protocol::ProtocolSandboxEnvironment;
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

        let mut report = TestRunReport::new();
        let mut initialized_protocols: HashMap<Protocol, ProtocolSandboxEnvironment> =
            HashMap::new();

        let protocols_dir = self.environment.input_dir.join("protocols");

        for protocol in &self.environment.protocols {
            if initialized_protocols.contains_key(protocol) {
                continue;
            }

            for sandbox_cfg in &self.config.protocol_sandboxes {
                let config_protocol = match sandbox_cfg {
                    ProtocolSandboxConfig::Stellar(_) => Protocol::Stellar,
                    ProtocolSandboxConfig::Near(_) => Protocol::Near,
                    ProtocolSandboxConfig::Icp(_) => Protocol::Icp,
                    ProtocolSandboxConfig::Ethereum(_) => Protocol::Ethereum,
                };

                if &config_protocol != protocol {
                    continue;
                }

                let sandbox_env = match sandbox_cfg {
                    ProtocolSandboxConfig::Stellar(config) => ProtocolSandboxEnvironment::Stellar(
                        StellarSandboxEnvironment::init(config.clone())?,
                    ),
                    ProtocolSandboxConfig::Near(config) => ProtocolSandboxEnvironment::Near(
                        NearSandboxEnvironment::init(config.clone()).await?,
                    ),
                    ProtocolSandboxConfig::Icp(config) => ProtocolSandboxEnvironment::Icp(
                        IcpSandboxEnvironment::init(config.clone())?,
                    ),
                    ProtocolSandboxConfig::Ethereum(config) => {
                        ProtocolSandboxEnvironment::Ethereum(EthereumSandboxEnvironment::init(
                            config.clone(),
                        )?)
                    }
                };

                initialized_protocols.insert(*protocol, sandbox_env);
                break;
            }
        }

        for protocol_name in &self.environment.protocols {
            let protocol_path = protocols_dir.join(protocol_name.as_str());

            if !protocol_path.is_dir() {
                self.environment.output_writer.write_str(&format!(
                    "No directory for protocol: {}",
                    protocol_name.as_str()
                ));
                continue;
            }

            let Some(sandbox) = initialized_protocols.get(&protocol_name) else {
                bail!(
                    "Sandbox not initialized for protocol: {}",
                    protocol_name.as_str()
                );
            };

            let mero = self.setup_mero(&sandbox.clone()).await?;

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
                    sandbox,
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
                    .write_header(&format!("Running protocol {}", sandbox.name()), 1);

                report = self
                    .run_scenarios(&mut ctx, report, app_name, scenario, &test_file_path)
                    .await?;

                self.environment
                    .output_writer
                    .write_header(&format!("Finished protocol {}", sandbox.name()), 1);
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

    async fn setup_mero(&self, sandbox: &ProtocolSandboxEnvironment) -> EyreResult<Mero> {
        self.environment
            .output_writer
            .write_header("Starting merod nodes", 2);

        let mut merods = HashMap::new();

        let swarm_host = self.config.network.swarm_host.to_string();
        let mut swarm_port = self.config.network.start_swarm_port;

        let server_host = self.config.network.server_host.to_string();
        let mut server_port = self.config.network.start_server_port;

        for i in 0..self.config.network.node_count {
            let node_name = format!("node{}", i + 1);
            if let HashMapEntry::Vacant(e) = merods.entry(node_name.clone()) {
                let config_args = [format!(
                    "discovery.rendezvous.namespace=\"calimero/e2e-tests/{}\"",
                    self.environment.test_id
                )];

                let node_args = sandbox.node_args(&node_name).await?;
                let config_args = config_args.iter().chain(node_args.iter());

                let merod = Merod::new(
                    node_name,
                    self.environment.nodes_dir.clone(),
                    &self.environment.logs_dir,
                    self.environment.merod_binary.clone(),
                    self.environment.output_writer,
                );

                let swarm_port =
                    PortBinding::next_available(self.config.network.swarm_host, &mut swarm_port)
                        .await?;

                let server_port =
                    PortBinding::next_available(self.config.network.server_host, &mut server_port)
                        .await?;

                merod
                    .init(
                        &swarm_host,
                        &server_host,
                        swarm_port.port(),
                        server_port.port(),
                        config_args.map(String::as_str),
                    )
                    .await?;

                let swarm_addr = swarm_port.into_socket_addr();
                let server_addr = server_port.into_socket_addr();

                merod.run().await?;

                let merod = e.insert(merod);

                while let Err(_) = try_join!(
                    TcpSocket::new_v4()?.connect(swarm_addr),
                    TcpSocket::new_v4()?.connect(server_addr)
                ) {
                    if let Some(exit_code) = merod.try_wait().await? {
                        bail!(
                            "merod process exited with code {} before becoming ready",
                            exit_code
                        );
                    }
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }

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

struct PortBinding {
    address: SocketAddr,
    listener: TcpListener,
}

impl PortBinding {
    async fn next_available(host: IpAddr, port: &mut u16) -> EyreResult<PortBinding> {
        for _ in 0..100 {
            let address = (host, *port).into();

            let res = TcpListener::bind(address).await;

            *port += 1;

            if let Ok(listener) = res {
                return Ok(PortBinding { address, listener });
            }
        }

        bail!(
            "unable to select a port in range {}..={}",
            *port - 100,
            *port - 1
        );
    }

    fn port(&self) -> u16 {
        self.address.port()
    }

    /// Drop the binding, returning the bound address.
    fn into_socket_addr(self) -> SocketAddr {
        drop(self.listener);
        self.address
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::net::IpAddr;

    use super::PortBinding;

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
