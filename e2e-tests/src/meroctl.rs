use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::{bail, eyre, OptionExt, Result as EyreResult};
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
            .run_cmd(node_name, &["app", "install", "--path", app_path])
            .await?;

        let data = self.remove_value_from_object(json, "data")?;
        let app_id = self.get_string_from_object(&data, "applicationId")?;

        Ok(app_id)
    }

    pub async fn context_create(
        &self,
        node_name: &str,
        app_id: &str,
    ) -> EyreResult<(String, String)> {
        let json = self
            .run_cmd(node_name, &["context", "create", "-a", app_id])
            .await?;

        let data = self.remove_value_from_object(json, "data")?;
        let context_id = self.get_string_from_object(&data, "contextId")?;
        let member_public_key = self.get_string_from_object(&data, "memberPublicKey")?;

        Ok((context_id, member_public_key))
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
                &[
                    "context",
                    "invite",
                    context_id,
                    inviteer_public_key,
                    invitee_public_key,
                ],
            )
            .await?;

        let data = self
            .remove_value_from_object(json, "data")?
            .as_str()
            .ok_or_eyre("data is not string")?
            .to_owned();

        Ok(data)
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
                &["context", "join", private_key, invitation_data],
            )
            .await?;

        let data = self.remove_value_from_object(json, "data")?;
        let context_id = self.get_string_from_object(&data, "contextId")?;
        let member_public_key = self.get_string_from_object(&data, "memberPublicKey")?;

        Ok((context_id, member_public_key))
    }

    pub async fn identity_generate(&self, node_name: &str) -> EyreResult<(String, String)> {
        let json = self.run_cmd(node_name, &["identity", "generate"]).await?;

        let data = self.remove_value_from_object(json, "data")?;
        let public_key = self.get_string_from_object(&data, "publicKey")?;
        let private_key = self.get_string_from_object(&data, "privateKey")?;

        Ok((public_key, private_key))
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
                &[
                    "json-rpc",
                    context_id,
                    method_name,
                    "--args-json",
                    &args_json,
                ],
            )
            .await?;

        if let Some(error) = json.get("error") {
            bail!("JSON RPC response error: {:?}", error)
        }

        Ok(json)
    }

    async fn run_cmd(&self, node_name: &str, args: &[&str]) -> EyreResult<serde_json::Value> {
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

    fn remove_value_from_object(
        &self,
        mut json: serde_json::Value,
        key: &str,
    ) -> EyreResult<serde_json::Value> {
        let json = json
            .as_object_mut()
            .ok_or_eyre("input is not a JSON object")?;

        json.remove(key)
            .ok_or_else(|| eyre!("'{}' not found in json object", key))
    }

    fn get_string_from_object(&self, json: &serde_json::Value, key: &str) -> EyreResult<String> {
        let json = json.as_object().ok_or_eyre("input is not a JSON object")?;

        let json = json
            .get(key)
            .ok_or_else(|| eyre!("'{}' not found in json object", key))?;

        let value = json
            .as_str()
            .ok_or_else(|| eyre!("'{}' is not a string", key))?;

        Ok(value.to_owned())
    }
}
