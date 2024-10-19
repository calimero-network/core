#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use core::future::{pending, Future};
use core::mem::replace;
use core::pin::Pin;
use core::str::FromStr;

use borsh::to_vec;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::config::ContextConfig;
use calimero_context::ContextManager;
use calimero_network::client::NetworkClient;
use calimero_network::config::NetworkConfig;
use calimero_network::types::{NetworkEvent, PeerId};
use calimero_node_primitives::{CallError, ExecutionRequest};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::events::{
    ApplicationEvent, ApplicationEventPayload, NodeEvent, OutcomeEvent, OutcomeEventPayload,
    PeerJoinedPayload, StateMutationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_runtime::logic::{Outcome, VMContext, VMLimits};
use calimero_runtime::Constraint;
use calimero_server::config::ServerConfig;
use calimero_storage::address::Id;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::Action;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::key::{
    ContextIdentity as ContextIdentityKey, ContextMeta as ContextMetaKey,
    ContextState as ContextStateKey, ContextTransaction as ContextTransactionKey,
};
use calimero_store::Store;
use camino::Utf8PathBuf;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::gossipsub::{IdentTopic, Message, TopicHash};
use libp2p::identity::Keypair;
use owo_colors::OwoColorize;
use serde_json::{
    from_slice as from_json_slice, from_str as from_json_str, to_vec as to_json_vec, Value,
};
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval_at, Instant};
use tokio::{select, spawn};
use tracing::{debug, error, info, warn};

use crate::runtime_compat::RuntimeCompatStore;
use crate::types::{ActionMessage, PeerAction, SyncMessage};

pub mod catchup;
pub mod runtime_compat;
pub mod types;

type BoxedFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[derive(Debug)]
#[non_exhaustive]
pub struct NodeConfig {
    pub home: Utf8PathBuf,
    pub identity: Keypair,
    pub network: NetworkConfig,
    pub datastore: StoreConfig,
    pub blobstore: BlobStoreConfig,
    pub context: ContextConfig,
    pub server: ServerConfig,
}

impl NodeConfig {
    #[must_use]
    pub const fn new(
        home: Utf8PathBuf,
        identity: Keypair,
        network: NetworkConfig,
        datastore: StoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            home,
            identity,
            network,
            datastore,
            blobstore,
            context,
            server,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    store: Store,
    ctx_manager: ContextManager,
    network_client: NetworkClient,
    node_events: broadcast::Sender<NodeEvent>,
}

pub async fn start(config: NodeConfig) -> EyreResult<()> {
    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let (node_events, _) = broadcast::channel(32);

    let (network_client, mut network_events) = calimero_network::run(&config.network).await?;

    let store = Store::open::<RocksDB>(&config.datastore)?;

    let blob_manager = BlobManager::new(store.clone(), FileSystem::new(&config.blobstore).await?);

    let (server_sender, mut server_receiver) = mpsc::channel(32);

    let ctx_manager = ContextManager::start(
        &config.context,
        store.clone(),
        blob_manager,
        server_sender.clone(),
        network_client.clone(),
    )
    .await?;

    let mut node = Node::new(
        &config,
        network_client.clone(),
        node_events.clone(),
        ctx_manager.clone(),
        store.clone(),
    );

    #[expect(trivial_casts, reason = "Necessary here")]
    let mut server = Box::pin(calimero_server::start(
        config.server,
        server_sender,
        ctx_manager,
        node_events,
        store,
    )) as BoxedFuture<EyreResult<()>>;

    let mut stdin = BufReader::new(stdin()).lines();

    match network_client
        .subscribe(IdentTopic::new("meta_topic"))
        .await
    {
        Ok(_) => info!("Subscribed to meta topic"),
        Err(err) => {
            error!("{}: {:?}", "Error subscribing to meta topic", err);
            bail!("Failed to subscribe to meta topic: {:?}", err)
        }
    };

    let mut catchup_interval_tick = interval_at(
        Instant::now()
            .checked_add(config.network.catchup.initial_delay)
            .ok_or_else(|| eyre!("Overflow when calculating initial catchup interval delay"))?,
        config.network.catchup.interval,
    );

    #[expect(clippy::redundant_pub_crate, reason = "Tokio code")]
    loop {
        select! {
            event = network_events.recv() => {
                let Some(event) = event else {
                    break;
                };
                node.handle_event(event).await?;
            }
            line = stdin.next_line() => {
                if let Some(line) = line? {
                    handle_line(&mut node, line).await?;
                }
            }
            result = &mut server => {
                result?;
                server = Box::pin(pending());
                continue;
            }
            Some(request) = server_receiver.recv() => node.handle_call(request).await,
            _ = catchup_interval_tick.tick() => node.handle_interval_catchup().await,
        }
    }

    Ok(())
}

// TODO: Consider splitting this long function into multiple parts.
#[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
#[expect(clippy::similar_names, reason = "Difference is clear enough")]
async fn handle_line(node: &mut Node, line: String) -> EyreResult<()> {
    let (command, args) = match line.split_once(' ') {
        Some((method, payload)) => (method, Some(payload)),
        None => (line.as_str(), None),
    };

    let ind = " │".yellow();

    // TODO: should be replaced with RPC endpoints
    match command {
        "call" => {
            if let Some((context_id, rest)) = args.and_then(|args| args.split_once(' ')) {
                let (method, rest) = rest.split_once(' ').unwrap_or((rest, "{}"));
                let (payload, executor_key) = rest.split_once(' ').unwrap_or((rest, ""));

                let (payload, executor_key) = match executor_key.parse::<PublicKey>() {
                    Ok(key) => (payload, key),
                    Err(err) => match payload.parse::<PublicKey>() {
                        Ok(key) => (rest, key),
                        Err(err_payload) => {
                            println!(
                                "{ind} Invalid executor public key: {}",
                                if executor_key.is_empty() {
                                    err_payload
                                } else {
                                    err
                                }
                            );
                            return Ok(());
                        }
                    },
                };

                if let Err(e) = from_json_str::<Value>(payload) {
                    println!("{ind} Failed to parse payload: {e}");
                };

                let (outcome_sender, outcome_receiver) = oneshot::channel();

                let Ok(context_id) = context_id.parse() else {
                    println!("{ind} Invalid context ID: {context_id}");
                    return Ok(());
                };

                let Ok(Some(context)) = node.ctx_manager.get_context(&context_id) else {
                    println!("{ind} Context not found: {context_id}");
                    return Ok(());
                };

                node.handle_call(ExecutionRequest::new(
                    context.id,
                    method.to_owned(),
                    payload.as_bytes().to_owned(),
                    executor_key,
                    outcome_sender,
                    None,
                ))
                .await;

                drop(spawn(async move {
                    if let Ok(outcome_result) = outcome_receiver.await {
                        println!("{ind}");

                        match outcome_result {
                            Ok(outcome) => {
                                match outcome.returns {
                                    Ok(result) => match result {
                                        Some(result) => {
                                            println!("{ind}   Return Value:");
                                            #[expect(
                                                clippy::option_if_let_else,
                                                reason = "Clearer here"
                                            )]
                                            let result = if let Ok(value) =
                                                from_json_slice::<Value>(&result)
                                            {
                                                format!(
                                                    "(json): {}",
                                                    format!("{value:#}")
                                                        .lines()
                                                        .map(|line| line.cyan().to_string())
                                                        .collect::<Vec<_>>()
                                                        .join("\n")
                                                )
                                            } else {
                                                format!("(raw): {:?}", result.cyan())
                                            };

                                            for line in result.lines() {
                                                println!("{ind}     > {line}");
                                            }
                                        }
                                        None => println!("{ind}   (No return value)"),
                                    },
                                    Err(err) => {
                                        let err = format!("{err:#?}");

                                        println!("{ind}   Error:");
                                        for line in err.lines() {
                                            println!("{ind}     > {}", line.yellow());
                                        }
                                    }
                                }

                                if !outcome.logs.is_empty() {
                                    println!("{ind}   Logs:");

                                    for log in outcome.logs {
                                        println!("{ind}     > {}", log.cyan());
                                    }
                                }
                            }
                            Err(err) => {
                                let err = format!("{err:#?}");

                                println!("{ind}   Error:");
                                for line in err.lines() {
                                    println!("{ind}     > {}", line.yellow());
                                }
                            }
                        }
                    }
                }));
            } else {
                println!(
                    "{ind} Usage: call <Context ID> <Method> <JSON Payload> <Executor Public Key<"
                );
            }
        }
        "peers" => {
            println!(
                "{ind} Peers (General): {:#?}",
                node.network_client.peer_count().await.cyan()
            );

            if let Some(args) = args {
                // TODO: implement print all and/or specific topic
                let topic = TopicHash::from_raw(args);
                println!(
                    "{ind} Peers (Session) for Topic {}: {:#?}",
                    topic.clone(),
                    node.network_client.mesh_peer_count(topic).await.cyan()
                );
            }
        }
        "store" => {
            // todo! revisit: get specific context state
            // todo! test this

            println!(
                "{ind} {c1:44} | {c2:44} | Value",
                c1 = "Context ID",
                c2 = "State Key",
            );

            let handle = node.store.handle();

            for (k, v) in handle.iter::<ContextStateKey>()?.entries() {
                let (k, v) = (k?, v?);
                let (cx, state_key) = (k.context_id(), k.state_key());
                let sk = Hash::from(state_key);
                let entry = format!("{c1:44} | {c2:44}| {c3:?}", c1 = cx, c2 = sk, c3 = v.value);
                for line in entry.lines() {
                    println!("{ind} {}", line.cyan());
                }
            }
        }
        "application" => 'done: {
            'usage: {
                let Some(args) = args else {
                    break 'usage;
                };

                let (subcommand, args) = args
                    .split_once(' ')
                    .map_or_else(|| (args, None), |(a, b)| (a, Some(b)));

                match subcommand {
                    "install" => {
                        let Some((type_, resource, metadata)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let type_ = iter.next()?;
                            let resource = iter.next()?;
                            let metadata = iter.next();

                            Some((type_, resource, metadata))
                        }) else {
                            println!(
                                "{ind} Usage: application install <\"url\"|\"file\"> <resource> [metadata]"
                            );
                            break 'done;
                        };

                        let application_id = match type_ {
                            "url" => {
                                let Ok(url) = resource.parse() else {
                                    println!("{ind} Invalid URL: {resource}");
                                    break 'done;
                                };

                                println!("{ind} Downloading application..");

                                node.ctx_manager
                                    .install_application_from_url(url, vec![])
                                    .await?
                            }
                            "file" => {
                                let path = Utf8PathBuf::from(resource);

                                node.ctx_manager
                                    .install_application_from_path(
                                        path,
                                        metadata
                                            .map(|x| x.as_bytes().to_owned())
                                            .unwrap_or_default(),
                                    )
                                    .await?
                            }
                            unknown => {
                                println!("{ind} Unknown resource type: `{unknown}`");
                                break 'done;
                            }
                        };

                        println!("{ind} Installed application: {application_id}");
                    }
                    "ls" => {
                        println!(
                            "{ind} {c1:44} | {c2:44} | Source",
                            c1 = "Application ID",
                            c2 = "Blob ID",
                        );

                        for application in node.ctx_manager.list_installed_applications()? {
                            let entry = format!(
                                "{c1:44} | {c2:44} | {c3}",
                                c1 = application.id,
                                c2 = application.blob,
                                c3 = application.source
                            );
                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                    // todo! a "show" subcommand should help keep "ls" compact
                    unknown => {
                        println!("{ind} Unknown command: `{unknown}`");
                        break 'usage;
                    }
                }

                break 'done;
            };
            println!("{ind} Usage: application [ls|install]");
        }
        "context" => 'done: {
            'usage: {
                let Some(args) = args else {
                    break 'usage;
                };

                let (subcommand, args) = args
                    .split_once(' ')
                    .map_or_else(|| (args, None), |(a, b)| (a, Some(b)));

                match subcommand {
                    "ls" => {
                        println!(
                            "{ind} {c1:44} | {c2:44} | Last Transaction",
                            c1 = "Context ID",
                            c2 = "Application ID",
                        );

                        let handle = node.store.handle();

                        for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                            let (k, v) = (k?, v?);
                            let (cx, app_id, last_tx) =
                                (k.context_id(), v.application.application_id(), v.root_hash);
                            let entry = format!(
                                "{c1:44} | {c2:44} | {c3}",
                                c1 = cx,
                                c2 = app_id,
                                c3 = Hash::from(last_tx)
                            );
                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                    "join" => {
                        let Some((private_key, invitation_payload)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let private_key = iter.next()?;
                            let invitation_payload = iter.next()?;

                            Some((private_key, invitation_payload))
                        }) else {
                            println!(
                                "{ind} Usage: context join <private_key> <invitation_payload>"
                            );
                            break 'done;
                        };

                        let Ok(private_key) = private_key.parse() else {
                            println!("{ind} Invalid private key: {private_key}");
                            break 'done;
                        };

                        let Ok(invitation_payload) = invitation_payload.parse() else {
                            println!("{ind} Invalid context ID: {private_key}");
                            break 'done;
                        };

                        let response = match node
                            .ctx_manager
                            .join_context(private_key, invitation_payload)
                            .await
                        {
                            Ok(response) => response,
                            Err(err) => {
                                println!("{ind} Unable to join context: {err}");
                                break 'done;
                            }
                        };

                        let Some((context_id, identity)) = response else {
                            println!("{ind} Unable to join context at this time, a catchup is in progress.");
                            break 'done;
                        };

                        println!(
                            "{ind} Joined context {context_id} as {identity}, waiting for catchup to complete..."
                        );
                    }
                    "leave" => {
                        let Some(context_id) = args else {
                            println!("{ind} Usage: context leave <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{ind} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let _ = node.ctx_manager.delete_context(&context_id).await?;

                        println!("{ind} Left context {context_id}");
                    }
                    "create" => {
                        let Some((application_id, context_seed, mut params)) =
                            args.and_then(|args| {
                                let mut iter = args.split(' ');
                                let application = iter.next()?;
                                let context_seed = iter.next();
                                let params = iter.next();
                                Some((application, context_seed, params))
                            })
                        else {
                            println!("{ind} Usage: context create <application_id> [context_seed] [initialization params]");
                            break 'done;
                        };

                        let Ok(application_id) = application_id.parse() else {
                            println!("{ind} Invalid application ID: {application_id}");
                            break 'done;
                        };

                        let (context_seed, params) = 'infer: {
                            let Some(context_seed) = context_seed else {
                                break 'infer (None, None);
                            };

                            if let Ok(context_seed) = context_seed.parse::<Hash>() {
                                break 'infer (Some(context_seed), params);
                            };

                            match replace(&mut params, Some(context_seed)).map(FromStr::from_str) {
                                Some(Ok(context_seed)) => {
                                    break 'infer (Some(context_seed), params)
                                }
                                None => break 'infer (None, params),
                                _ => {}
                            };

                            println!("{ind} Invalid context seed: {context_seed}");
                            break 'done;
                        };

                        let (context_id, identity) = node
                            .ctx_manager
                            .create_context(
                                context_seed.map(Into::into),
                                application_id,
                                None,
                                params.map(|x| x.as_bytes().to_owned()).unwrap_or_default(),
                            )
                            .await?;

                        println!("{ind} Created context {context_id} with identity {identity}");
                    }
                    "invite" => {
                        let Some((context_id, inviter_id, invitee_id)) = args.and_then(|args| {
                            let mut iter = args.split(' ');
                            let context_id = iter.next()?;
                            let inviter_id = iter.next()?;
                            let invitee_id = iter.next()?;
                            Some((context_id, inviter_id, invitee_id))
                        }) else {
                            println!(
                                "{ind} Usage: context invite <context_id> <inviter_id> <invitee_id>"
                            );
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{ind} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let Ok(inviter_id) = inviter_id.parse() else {
                            println!("{ind} Invalid public key for inviter: {inviter_id}");
                            break 'done;
                        };

                        let Ok(invitee_id) = invitee_id.parse() else {
                            println!("{ind} Invalid public key for invitee: {invitee_id}");
                            break 'done;
                        };

                        let Some(invitation_payload) = node
                            .ctx_manager
                            .invite_to_context(context_id, inviter_id, invitee_id)
                            .await?
                        else {
                            println!("{ind} Unable to invite {invitee_id} to context {context_id}");
                            break 'done;
                        };

                        println!("{ind} Invited {invitee_id} to context {context_id}");
                        println!("{ind} Invitation Payload: {invitation_payload}");
                    }
                    "delete" => {
                        let Some(context_id) = args else {
                            println!("{ind} Usage: context delete <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{ind} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let _ = node.ctx_manager.delete_context(&context_id).await?;

                        println!("{ind} Deleted context {context_id}");
                    }
                    "identity" => {
                        let Some(args) = args else {
                            println!(
                                "{ind} Usage: context identity [ls <context_id>|new <context_id>]"
                            );
                            break 'done;
                        };

                        let (subcommand, args) = args
                            .split_once(' ')
                            .map_or_else(|| (args, None), |(a, b)| (a, Some(b)));

                        match subcommand {
                            "ls" => {
                                let Some(context_id) = args else {
                                    println!("{ind} Usage: context identity ls <context_id>");
                                    break 'done;
                                };

                                let Ok(context_id) = context_id.parse() else {
                                    println!("{ind} Invalid context ID: {context_id}");
                                    break 'done;
                                };

                                let handle = node.store.handle();

                                let mut iter = handle.iter::<ContextIdentityKey>()?;

                                let first = 'first: {
                                    let Some(k) = iter
                                        .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
                                        .transpose()
                                    else {
                                        break 'first None;
                                    };

                                    Some((k, iter.read()))
                                };

                                println!("{ind} {c1:44} | Owned", c1 = "Identity");

                                for (k, v) in first.into_iter().chain(iter.entries()) {
                                    let (k, v) = (k?, v?);
                                    let entry = format!(
                                        "{c1:44} | {}",
                                        if v.private_key.is_some() { "*" } else { " " },
                                        c1 = k.public_key(),
                                    );
                                    for line in entry.lines() {
                                        println!("{ind} {}", line.cyan());
                                    }
                                }
                            }
                            "new" => {
                                let identity = node.ctx_manager.new_identity();

                                println!("{ind} Private Key: {}", identity.cyan());
                                println!("{ind} Public Key: {}", identity.public_key().cyan());
                            }
                            unknown => {
                                println!("{ind} Unknown command: `{unknown}`");
                                println!("{ind} Usage: context identity [ls <context_id>|new]");
                                break 'done;
                            }
                        }
                    }
                    "transactions" => {
                        let Some(context_id) = args else {
                            println!("{ind} Usage: context transactions <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{ind} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let handle = node.store.handle();

                        let mut iter = handle.iter::<ContextTransactionKey>()?;

                        let first = 'first: {
                            let Some(k) = iter
                                .seek(ContextTransactionKey::new(context_id, [0; 32]))
                                .transpose()
                            else {
                                break 'first None;
                            };

                            Some((k, iter.read()))
                        };

                        println!("{ind} {c1:44} | {c2:44}", c1 = "Hash", c2 = "Prior Hash");

                        for (k, v) in first.into_iter().chain(iter.entries()) {
                            let (k, v) = (k?, v?);
                            let entry = format!(
                                "{c1:44} | {c2}",
                                c1 = Hash::from(k.transaction_id()),
                                c2 = Hash::from(v.prior_hash),
                            );
                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                    "state" => {
                        let Some(context_id) = args else {
                            println!("{ind} Usage: context state <context_id>");
                            break 'done;
                        };

                        let Ok(context_id) = context_id.parse() else {
                            println!("{ind} Invalid context ID: {context_id}");
                            break 'done;
                        };

                        let handle = node.store.handle();

                        println!("{ind} {c1:44} | {c2:44}", c1 = "State Key", c2 = "Value");

                        let mut iter = handle.iter::<ContextStateKey>()?;

                        // let first = 'first: {
                        //     let Some(k) = iter
                        //         .seek(ContextStateKey::new(context_id, [0; 32]))
                        //         .transpose()
                        //     else {
                        //         break 'first None;
                        //     };

                        //     Some((k, iter.read()))
                        //                   ^^^^~ ContextState<'a> lends the `iter`, while `.entries()` attempts to mutate it
                        // };

                        for (k, v) in iter.entries() {
                            let (k, v) = (k?, v?);
                            if k.context_id() != context_id {
                                // todo! revisit this when DBIter::seek no longer returns
                                // todo! the sought item, you have to call next(), read()
                                continue;
                            }
                            let entry = format!(
                                "{c1:44} | {c2:?}",
                                c1 = Hash::from(k.state_key()),
                                c2 = v.value,
                            );
                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                    unknown => {
                        println!("{ind} Unknown command: `{unknown}`");
                        break 'usage;
                    }
                }

                break 'done;
            };
            println!(
                "{ind} Usage: context [ls|join|leave|invite|create|delete|state|identity] [args]"
            );
        }
        unknown => {
            println!("{ind} Unknown command: `{unknown}`");
            println!("{ind} Usage: [call|peers|pool|gc|store|context|application] [args]");
        }
    }

    Ok(())
}

impl Node {
    #[must_use]
    pub const fn new(
        _config: &NodeConfig,
        network_client: NetworkClient,
        node_events: broadcast::Sender<NodeEvent>,
        ctx_manager: ContextManager,
        store: Store,
    ) -> Self {
        Self {
            store,
            ctx_manager,
            network_client,
            node_events,
        }
    }

    pub async fn handle_event(&mut self, event: NetworkEvent) -> EyreResult<()> {
        match event {
            NetworkEvent::Subscribed {
                peer_id: their_peer_id,
                topic: topic_hash,
            } => {
                if let Err(err) = self.handle_subscribed(their_peer_id, &topic_hash) {
                    error!(?err, "Failed to handle subscribed event");
                }
            }
            NetworkEvent::Message { message, .. } => {
                if let Err(err) = self.handle_message(message).await {
                    error!(?err, "Failed to handle message event");
                }
            }
            NetworkEvent::ListeningOn { address, .. } => {
                info!("Listening on: {}", address);
            }
            NetworkEvent::StreamOpened { peer_id, stream } => {
                info!("Stream opened from peer: {}", peer_id);

                if let Err(err) = self.handle_opened_stream(stream).await {
                    error!(?err, "Failed to handle stream");
                }

                info!("Stream closed from peer: {:?}", peer_id);
            }
            _ => error!("Unhandled event: {:?}", event),
        }

        Ok(())
    }

    fn handle_subscribed(&self, their_peer_id: PeerId, topic_hash: &TopicHash) -> EyreResult<()> {
        let Ok(context_id) = topic_hash.as_str().parse() else {
            // bail!(
            //     "Failed to parse topic hash '{}' into context ID",
            //     topic_hash
            // );
            return Ok(());
        };

        let handle = self.store.handle();

        if !handle.has(&ContextMetaKey::new(context_id))? {
            debug!(
                %context_id,
                %their_peer_id,
                "Observed subscription to unknown context, ignoring.."
            );
            return Ok(());
        };

        info!("{} joined the session.", their_peer_id.cyan());
        drop(
            self.node_events
                .send(NodeEvent::Application(ApplicationEvent::new(
                    context_id,
                    ApplicationEventPayload::PeerJoined(PeerJoinedPayload::new(their_peer_id)),
                ))),
        );

        Ok(())
    }

    async fn handle_message(&mut self, message: Message) -> EyreResult<()> {
        let Some(source) = message.source else {
            warn!(?message, "Received message without source");
            return Ok(());
        };

        match from_json_slice(&message.data)? {
            PeerAction::ActionList(action_list) => {
                debug!(?action_list, %source, "Received action list");

                for action in action_list.actions {
                    debug!(?action, %source, "Received action");
                    let Some(mut context) =
                        self.ctx_manager.get_context(&action_list.context_id)?
                    else {
                        bail!("Context '{}' not found", action_list.context_id);
                    };
                    match action {
                        Action::Compare { id } => {
                            self.send_comparison_message(&mut context, id, action_list.public_key)
                                .await
                        }
                        Action::Add { .. } | Action::Delete { .. } | Action::Update { .. } => {
                            self.apply_action(&mut context, &action, action_list.public_key)
                                .await
                        }
                    }?;
                }
                Ok(())
            }
            PeerAction::Sync(sync) => {
                debug!(?sync, %source, "Received sync request");

                let Some(mut context) = self.ctx_manager.get_context(&sync.context_id)? else {
                    bail!("Context '{}' not found", sync.context_id);
                };
                let outcome = self
                    .compare_trees(&mut context, &sync.comparison, sync.public_key)
                    .await?;

                match outcome.returns {
                    Ok(Some(actions_data)) => {
                        let (local_actions, remote_actions): (Vec<Action>, Vec<Action>) =
                            from_json_slice(&actions_data)?;

                        // Apply local actions
                        for action in local_actions {
                            match action {
                                Action::Compare { id } => {
                                    self.send_comparison_message(&mut context, id, sync.public_key)
                                        .await
                                }
                                Action::Add { .. }
                                | Action::Delete { .. }
                                | Action::Update { .. } => {
                                    self.apply_action(&mut context, &action, sync.public_key)
                                        .await
                                }
                            }?;
                        }

                        if !remote_actions.is_empty() {
                            // Send remote actions back to the peer
                            // TODO: This just sends one at present - needs to send a batch
                            let new_message = ActionMessage {
                                actions: remote_actions,
                                context_id: sync.context_id,
                                public_key: sync.public_key,
                                root_hash: context.root_hash,
                            };
                            self.push_action(sync.context_id, PeerAction::ActionList(new_message))
                                .await?;
                        }
                    }
                    Ok(None) => {
                        // No actions needed
                    }
                    Err(err) => {
                        error!("Error during comparison: {err:?}");
                        // TODO: Handle the error appropriately
                    }
                }
                Ok(())
            }
        }
    }

    async fn send_comparison_message(
        &mut self,
        context: &mut Context,
        id: Id,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let compare_outcome = self
            .generate_comparison_data(context, id, public_key)
            .await?;
        match compare_outcome.returns {
            Ok(Some(comparison_data)) => {
                // Generate a new Comparison for this entity and send it to the peer
                let new_sync = SyncMessage {
                    comparison: from_json_slice(&comparison_data)?,
                    context_id: context.id,
                    public_key,
                    root_hash: context.root_hash,
                };
                self.push_action(context.id, PeerAction::Sync(new_sync))
                    .await?;
                Ok(())
            }
            Ok(None) => Err(eyre!("No comparison data generated")),
            Err(err) => Err(eyre!(err)),
        }
    }

    async fn push_action(&self, context_id: ContextId, action: PeerAction) -> EyreResult<()> {
        drop(
            self.network_client
                .publish(TopicHash::from_raw(context_id), to_json_vec(&action)?)
                .await?,
        );

        Ok(())
    }

    pub async fn handle_call(&mut self, request: ExecutionRequest) {
        let Ok(Some(mut context)) = self.ctx_manager.get_context(&request.context_id) else {
            drop(request.outcome_sender.send(Err(CallError::ContextNotFound {
                context_id: request.context_id,
            })));
            return;
        };

        let task = self.call_query(
            &mut context,
            request.method,
            request.payload,
            request.executor_public_key,
        );

        drop(request.outcome_sender.send(task.await.map_err(|err| {
            error!(%err, "failed to execute local query");

            CallError::InternalError
        })));
    }

    async fn call_query(
        &mut self,
        context: &mut Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> Result<Outcome, CallError> {
        let outcome_option = self
            .execute(context, method, payload, executor_public_key)
            .await
            .map_err(|e| {
                error!(%e, "Failed to execute query call.");
                CallError::InternalError
            })?;

        let Some(outcome) = outcome_option else {
            return Err(CallError::ApplicationNotInstalled {
                application_id: context.application_id,
            });
        };

        if self
            .network_client
            .mesh_peer_count(TopicHash::from_raw(context.id))
            .await
            != 0
        {
            let actions = outcome
                .actions
                .iter()
                .map(|a| borsh::from_slice(a))
                .collect::<Result<Vec<Action>, _>>()
                .map_err(|err| {
                    error!(%err, "Failed to deserialize actions.");
                    CallError::InternalError
                })?;

            self.push_action(
                context.id,
                PeerAction::ActionList(ActionMessage {
                    actions,
                    context_id: context.id,
                    public_key: executor_public_key,
                    root_hash: context.root_hash,
                }),
            )
            .await
            .map_err(|err| {
                error!(%err, "Failed to push action over the network.");
                CallError::InternalError
            })?;
        }

        Ok(outcome)
    }

    async fn apply_action(
        &mut self,
        context: &mut Context,
        action: &Action,
        public_key: PublicKey,
    ) -> EyreResult<()> {
        let outcome = self
            .execute(
                context,
                "apply_action".to_owned(),
                to_vec(action)?,
                public_key,
            )
            .await
            .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))?;
        drop(outcome.returns?);
        Ok(())
    }

    async fn compare_trees(
        &mut self,
        context: &mut Context,
        comparison: &Comparison,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.execute(
            context,
            "compare_trees".to_owned(),
            to_vec(comparison)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn generate_comparison_data(
        &mut self,
        context: &mut Context,
        id: Id,
        public_key: PublicKey,
    ) -> EyreResult<Outcome> {
        self.execute(
            context,
            "generate_comparison_data".to_owned(),
            to_vec(&id)?,
            public_key,
        )
        .await
        .and_then(|outcome| outcome.ok_or_else(|| eyre!("Application not installed")))
    }

    async fn execute(
        &mut self,
        context: &mut Context,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> EyreResult<Option<Outcome>> {
        let mut storage = RuntimeCompatStore::new(&mut self.store, context.id);

        let Some(blob) = self
            .ctx_manager
            .load_application_blob(&context.application_id)
            .await?
        else {
            return Ok(None);
        };

        let outcome = calimero_runtime::run(
            &blob,
            &method,
            VMContext::new(payload, *executor_public_key),
            &mut storage,
            &get_runtime_limits()?,
        )?;

        if outcome.returns.is_ok() {
            if let Some(root_hash) = outcome.root_hash {
                if outcome.actions.is_empty() {
                    eyre::bail!("Context state changed, but no actions were generated, discarding execution outcome to mitigate potential state inconsistency");
                }

                context.root_hash = root_hash.into();

                drop(
                    self.node_events
                        .send(NodeEvent::Application(ApplicationEvent::new(
                            context.id,
                            ApplicationEventPayload::StateMutation(StateMutationPayload::new(
                                context.root_hash,
                            )),
                        ))),
                );

                self.ctx_manager.save_context(context)?;
            }

            if !storage.is_empty() {
                storage.commit()?;
            }

            drop(
                self.node_events
                    .send(NodeEvent::Application(ApplicationEvent::new(
                        context.id,
                        ApplicationEventPayload::OutcomeEvent(OutcomeEventPayload::new(
                            outcome
                                .events
                                .iter()
                                .map(|e| OutcomeEvent::new(e.kind.clone(), e.data.clone()))
                                .collect(),
                        )),
                    ))),
            );
        }

        Ok(Some(outcome))
    }
}

// TODO: move this into the config
// TODO: also this would be nice to have global default with per application customization
fn get_runtime_limits() -> EyreResult<VMLimits> {
    Ok(VMLimits::new(
        /*max_stack_size:*/ 200 << 10, // 200 KiB
        /*max_memory_pages:*/ 1 << 10, // 1 KiB
        /*max_registers:*/ 100,
        /*max_register_size:*/ (100 << 20).validate()?, // 100 MiB
        /*max_registers_capacity:*/ 1 << 30, // 1 GiB
        /*max_logs:*/ 100,
        /*max_log_size:*/ 16 << 10, // 16 KiB
        /*max_events:*/ 100,
        /*max_event_kind_size:*/ 100,
        /*max_event_data_size:*/ 16 << 10, // 16 KiB
        /*max_storage_key_size:*/ (1 << 20).try_into()?, // 1 MiB
        /*max_storage_value_size:*/
        (10 << 20).try_into()?, // 10 MiB
                                // can_write: writes, // todo!
    ))
}
