use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::{bail, eyre, OptionExt, Result as EyreResult};
use tokio::process::Command;

use crate::output::OutputWriter;

pub struct Meroctl {
    home_dir: Utf8PathBuf,
    binary: Utf8PathBuf,
    output_writer: OutputWriter,
}

impl Meroctl {
    pub const fn new(
        home_dir: Utf8PathBuf,
        binary: Utf8PathBuf,
        output_writer: OutputWriter,
    ) -> Self {
        Self {
            home_dir,
            binary,
            output_writer,
        }
    }

    pub async fn application_install(&self, node_name: &str, app_path: &str) -> EyreResult<String> {
        let json = self
            .run_cmd(node_name, ["app", "install", "--path", app_path])
            .await?;

        let data = self.remove_value_from_object(json, "data")?;
        let app_id = self.get_string_from_object(&data, "applicationId")?;

        Ok(app_id)
    }

    pub async fn application_get(
        &self,
        node_name: &str,
        app_id: &str,
    ) -> EyreResult<serde_json::Value> {
        let json = self.run_cmd(node_name, ["app", "get", app_id]).await?;

        let data = self.remove_value_from_object(json, "data")?;
        let application = self.remove_value_from_object(data, "application")?;

        Ok(application)
    }

    pub async fn context_create(
        &self,
        node_name: &str,
        app_id: &str,
        protocol_name: &str,
    ) -> EyreResult<(String, String)> {
        let json = self
            .run_cmd(
                node_name,
                [
                    "context",
                    "create",
                    "-a",
                    app_id,
                    "--protocol",
                    protocol_name,
                ],
            )
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
        inviter_public_key: &str,
        invitee_public_key: &str,
    ) -> EyreResult<String> {
        let json = self
            .run_cmd(
                node_name,
                [
                    "context",
                    "invite",
                    context_id,
                    invitee_public_key,
                    "--as",
                    inviter_public_key,
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
            .run_cmd(node_name, ["context", "join", private_key, invitation_data])
            .await?;

        let data = self.remove_value_from_object(json, "data")?;
        let context_id = self.get_string_from_object(&data, "contextId")?;
        let member_public_key = self.get_string_from_object(&data, "memberPublicKey")?;

        Ok((context_id, member_public_key))
    }

    pub async fn identity_generate(&self, node_name: &str) -> EyreResult<(String, String)> {
        let json = self.run_cmd(node_name, ["identity", "generate"]).await?;

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
        args: &serde_json::Value,
        public_key: &str,
    ) -> EyreResult<serde_json::Value> {
        let args_json = serde_json::to_string(args)?;
        let json = self
            .run_cmd(
                node_name,
                [
                    "call",
                    context_id,
                    method_name,
                    "--args",
                    &args_json,
                    "--as",
                    public_key,
                ],
            )
            .await?;

        if let Some(error) = json.get("error") {
            bail!("JSON RPC response error: {:?}", error)
        }

        Ok(json)
    }

    async fn run_cmd<'a>(
        &'a self,
        node_name: &'a str,
        args: impl IntoIterator<Item = &'a str>,
    ) -> EyreResult<serde_json::Value> {
        let mut command = Command::new(&self.binary);

        let mut command_line = format!("Command: '{}", &self.binary);

        let root_args = [
            "--home",
            self.home_dir.as_str(),
            "--node-name",
            node_name,
            "--output-format",
            "json",
        ];

        for arg in root_args.into_iter().chain(args) {
            let _ignored = command.arg(arg);
            command_line.reserve(arg.len() + 1);
            command_line.push(' ');
            command_line.push_str(arg);
        }

        command_line.push('\'');

        self.output_writer.write_str(&command_line);

        let output = command
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
        let Some(json) = json.as_object_mut() else {
            bail!("'{}' is not a JSON object", json)
        };

        json.remove(key)
            .ok_or_else(|| eyre!("key '{}' not found in '{:?}' JSON object", key, json))
    }

    fn get_string_from_object(&self, json: &serde_json::Value, key: &str) -> EyreResult<String> {
        let Some(json) = json.as_object() else {
            bail!("'{}' is not a JSON object", json)
        };

        let json = json
            .get(key)
            .ok_or_else(|| eyre!("key '{}' not found in '{:?}' JSON object", key, json))?;

        let value = json
            .as_str()
            .ok_or_else(|| eyre!("'{}' is not a string", key))?;

        Ok(value.to_owned())
    }
}
