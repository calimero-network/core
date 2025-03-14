use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::CallContextResponse;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use libp2p::Multiaddr;
use regex::Regex;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType, lookup_alias};
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Call a context function")]
pub struct CallCommand {
    #[clap(help = "The context ID or alias to call")]
    context: String,

    #[clap(help = "The function to call")]
    function: String,

    #[clap(help = "The parameters to pass to the function. Supports alias substitution with %alias% syntax")]
    params: Option<String>,

    #[clap(long = "as", help = "The identity alias to use for the call")]
    identity: Option<Alias<PublicKey>>,
}

impl Report for CallContextResponse {
    fn report(&self) {
        if let Some(result) = &self.data.result {
            println!("Result: {}", String::from_utf8_lossy(result));
        } else {
            println!("No result returned");
        }
    }
}

impl CallCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        // Resolve context ID if it's an alias
        let context_id = if let Ok(context_id) = self.context.parse::<ContextId>() {
            context_id
        } else if let Ok(alias) = self.context.parse::<Alias<ContextId>>() {
            match lookup_alias(multiaddr.clone(), &config.identity, alias, None).await {
                Ok(response) => {
                    if let Some(context_id) = response.data.value {
                        context_id
                    } else {
                        bail!("Context alias '{}' not found", self.context);
                    }
                }
                Err(e) => bail!("Error looking up context alias '{}': {}", self.context, e),
            }
        } else {
            bail!("Invalid context ID or alias format: {}", self.context);
        };

        // Process parameters for alias substitution if needed
        let processed_params = match self.params {
            Some(p) if p.contains('%') => {
                Some(substitute_aliases(environment, &client, &multiaddr, &config.identity, &p).await?)
            }
            p => p,
        };

        // Call the context function
        call_context(
            environment,
            &client,
            &multiaddr,
            context_id,
            &self.function,
            processed_params,
            &config.identity,
            self.identity,
        )
        .await?;

        Ok(())
    }
}

async fn call_context(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    function: &str,
    params: Option<String>,
    keypair: &libp2p::identity::Keypair,
    identity: Option<Alias<PublicKey>>,
) -> EyreResult<()> {
    // Resolve identity if provided
    let member_public_key = if let Some(alias) = identity {
        match lookup_alias(base_multiaddr.clone(), keypair, alias, None).await {
            Ok(response) => {
                if let Some(public_key) = response.data.value {
                    Some(public_key)
                } else {
                    bail!("Identity alias not found");
                }
            }
            Err(e) => bail!("Error looking up identity alias: {}", e),
        }
    } else {
        None
    };

    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/call/{function}"),
    )?;

    // Create request with parameters and optional identity
    let request = serde_json::json!({
        "params": params.map(String::into_bytes).unwrap_or_default(),
        "member_public_key": member_public_key,
    });

    let response: CallContextResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(())
}

/// Substitutes aliases in the format %alias% with their corresponding public keys
async fn substitute_aliases(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    keypair: &libp2p::identity::Keypair,
    params: &str,
) -> EyreResult<String> {
    let re = Regex::new(r"%([^%]+)%")?;
    let mut result = params.to_string();
    
    for cap in re.captures_iter(params) {
        if let Some(alias_match) = cap.get(1) {
            let alias_str = alias_match.as_str();
            // Parse the alias string into an Alias<PublicKey>
            if let Ok(alias) = alias_str.parse::<Alias<PublicKey>>() {
                // Look up the alias to get the public key
                match lookup_alias(base_multiaddr.clone(), keypair, alias, None).await {
                    Ok(response) => {
                        if let Some(public_key) = response.data.value {
                            // Replace the %alias% with the public key
                            result = result.replace(&format!("%{}%", alias_str), &public_key.to_string());
                            environment.output.write(&InfoLine(&format!(
                                "Substituted alias '{}' with public key '{}'",
                                alias_str, public_key
                            )));
                        } else {
                            environment.output.write(&ErrorLine(&format!(
                                "Alias '{}' not found, leaving as is",
                                alias_str
                            )));
                        }
                    }
                    Err(e) => {
                        environment.output.write(&ErrorLine(&format!(
                            "Error looking up alias '{}': {}, leaving as is",
                            alias_str, e
                        )));
                    }
                }
            } else {
                environment.output.write(&ErrorLine(&format!(
                    "Invalid alias format '{}', leaving as is",
                    alias_str
                )));
            }
        }
    }
    
    Ok(result)
} 