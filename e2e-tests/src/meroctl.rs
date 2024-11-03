use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::Result as EyreResult;
use tokio::process::Command;

use crate::TestEnvironment;

pub struct Meroctl {
    nodes_dir: Utf8PathBuf,
    binary: Utf8PathBuf,
}

impl Meroctl {
    pub fn new(environment: &TestEnvironment) -> Self {
        Self {
            nodes_dir: environment.nodes_dir.clone(),
            binary: environment.meroctl_binary.clone(),
        }
    }

    pub async fn application_install(&self, node_name: &str, app_path: &str) -> EyreResult<String> {
        let json = self
            .run_cmd(node_name, Box::new(["app", "install", "--path", app_path]))
            .await?;

        let app_id = json["data"]["applicationId"]
            .as_str()
            .expect("data.applicationId not found");

        Ok(app_id.to_owned())
    }

    pub async fn context_create(
        &self,
        node_name: &str,
        app_id: &str,
    ) -> EyreResult<(String, String)> {
        let json = self
            .run_cmd(node_name, Box::new(["context", "create", "-a", app_id]))
            .await?;

        let context_id = json["data"]["contextId"]
            .as_str()
            .expect("data.contextId not found");
        let member_public_key = json["data"]["memberPublicKey"]
            .as_str()
            .expect("data.memberPublicKey not found");

        Ok((context_id.to_owned(), member_public_key.to_owned()))
    }

    pub async fn context_invite(
        &self,
        node_name: &str,
        context_id: &str,
        inviteer_public_key: &str,
        invitee_public_key: &str,
    ) -> EyreResult<String> {
        let json = self
            .run_cmd(
                node_name,
                Box::new([
                    "context",
                    "invite",
                    context_id,
                    inviteer_public_key,
                    invitee_public_key,
                ]),
            )
            .await?;

        let data = json["data"]
            .as_str()
            .expect("Invite response data not found");

        Ok(data.to_owned())
    }

    pub async fn context_join(
        &self,
        node_name: &str,
        private_key: &str,
        invitation_data: &str,
    ) -> EyreResult<(String, String)> {
        let json = self
            .run_cmd(
                node_name,
                Box::new(["context", "join", private_key, invitation_data]),
            )
            .await?;

        let context_id = json["data"]["contextId"]
            .as_str()
            .expect("data.contextId not found");
        let member_public_key = json["data"]["memberPublicKey"]
            .as_str()
            .expect("data.memberPublicKey not found");

        Ok((context_id.to_owned(), member_public_key.to_owned()))
    }

    pub async fn identity_generate(&self, node_name: &str) -> EyreResult<(String, String)> {
        let json = self
            .run_cmd(node_name, Box::new(["identity", "generate"]))
            .await?;

        let public_key = json["data"]["publicKey"]
            .as_str()
            .expect("data.publicKey not found");
        let private_key = json["data"]["privateKey"]
            .as_str()
            .expect("data.privateKey not found");

        Ok((public_key.to_owned(), private_key.to_owned()))
    }

    pub async fn json_rpc_execute(
        &self,
        node_name: &str,
        context_id: &str,
        method_name: &str,
        args_json: &serde_json::Value,
    ) -> EyreResult<serde_json::Value> {
        let args_json = serde_json::to_string(args_json)?;
        let json = self
            .run_cmd(
                node_name,
                Box::new([
                    "json-rpc",
                    context_id,
                    method_name,
                    "--args-json",
                    &args_json,
                ]),
            )
            .await?;

        println!("{:?}", json);

        Ok(json)
    }

    async fn run_cmd(&self, node_name: &str, args: Box<[&str]>) -> EyreResult<serde_json::Value> {
        let mut root_args = vec![
            "--home",
            self.nodes_dir.as_str(),
            "--node-name",
            node_name,
            "--output-format",
            "json",
        ];

        root_args.extend(args);

        println!("Running command '{:}' {:?}", &self.binary, root_args);

        let output = Command::new(&self.binary)
            .args(root_args)
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
