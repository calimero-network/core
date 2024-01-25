#!/usr/bin/env node
import yargs from "yargs";
import { hideBin } from "yargs/helpers";
import { JoinCommandOptions, StartSessionCommandOptions, AddKeysCommandOptions } from "./types";
import * as fs from "fs";
import open from "open";

// TODO - update output with right chalk colors

//TOOD - does user connect to network or does he just reveals his IP and Port in Node Discovery?
// After "Join" process is finished the handler is considered complete.
// join arguments (addr, port) refer to known peer or boostrap server in the network
// *add more arguments if needed
const join = {
	command: "join",
	describe: "join P2P network on specific address",
	builder: (yargs: yargs.Argv) => {
		return yargs
			.usage("Usage: core-cli join --address <address> [--port <port>] [--token <token>]")
			.option("address", {
				alias: "a",
				describe: "Network address to join",
				demandOption: true,
				type: "string"
			})
			.option("port", {
				alias: "p",
				describe: "Port number to connect to",
				type: "number"
			})
			.option("token", {
				alias: "t",
				describe: "Authentication token if required",
				type: "string"
			})
			.check((argv) => {
				if (!argv.address) {
					throw new Error("The --address option is required");
				}
				return true;
			});
	},
	handler: async (argv: JoinCommandOptions) => {
		console.log(`Joining network at address ${argv.address}:${argv.port || 3000}`);
		if (argv.token) {
			// TODO
			//const isTokenValid = await verifyAuthToken(arvg.token);
			// if (isTokenValid) {
			// 	console.log(`Using authentication token: ${argv.token}`);
			// 	await JoinP2PNetwork(arvg.address, argv.port, arg.token);
			// } else {
			// 	console.log(chalk.red(`Authentication token not valid: ${argv.token}`));
			// }
			console.log(`Using authentication token: ${argv.token}`);
		}
	}
};

//TODO - Period during which user interacts with the application
// Suppose in P2P Chat - users are connected and that is one Application sessions
// Config file - TODO what is it, is it needed, ...
const startSession = {
	command: "start-session",
	describe: "Start an Application Session",
	builder: (yargs: yargs.Argv) => {
		return yargs
			.usage("Usage: core-cli start-session --app <appName> [--config <configFilePath>]")
			.option("app", {
				alias: "a",
				describe: "Name of the application to start",
				demandOption: true,
				type: "string"
			})
			.option("config", {
				alias: "c",
				describe: "Path to configuration file (optional)",
				type: "string"
			})
			.check((argv) => {
				if (!argv.app) {
					throw new Error("The --app option is required");
				}
				return true;
			});
	},
	handler: (argv: StartSessionCommandOptions) => {
		console.log(`Starting Application Session for ${argv.app}`);
		if (argv.config) {
			console.log(`Starting session using configuration file: ${argv.config}`);
			//TODO
			//await startApplicationSession(arvg.app, argv.config);
		} else {
			//TODO
			console.log(`Starting session with default configuration for ${argv.app}`);
			//await startApplicationSession(argv.app);
		}
		// Active session loop - testing only
		// console.log("The application session is running... (Press Ctrl+C to stop)");
		// const intervalId = setInterval(() => {
		// 	console.log("Session is active...");
		// }, 10000);
		// process.on("SIGINT", () => {
		// 	clearInterval(intervalId);
		// 	console.log("Application session ended.");
		// 	process.exit(0);
		// });
	}
};


// TODO - Define what is a keypair, which algorithm
// Define where should storage be to save there keys
// extract data from string or from json file then create file in XYZ location
// should be similar what near does with ~./near-credentials/ or Keychain
// Keys used for signing transactions and P2P encryption
const addKeyPair = {
	command: "add-keypair",
	describe: "Support for importing raw key pairs",
	builder: (yargs: yargs.Argv) => {
		return yargs
			.option("keys", {
				alias: "k",
				describe: "Key pair as a JSON string",
				type: "string"
			})
			.option("file", {
				alias: "f",
				describe: "Path to a JSON file containing the keys",
				type: "string"
			})
			.check((argv) => {
				if (!argv.keys && !argv.file) {
					throw new Error("Either --keys or --file must be provided");
				}
				if (argv.keys && argv.file) {
					throw new Error("Only one of --keys or --file should be provided");
				}
				return true;
			});
	},
	handler: (argv: AddKeysCommandOptions) => {
		let keyData;
		if (argv.keys) {
			try {
				keyData = JSON.parse(argv.keys);
			} catch (error) {
				console.error("Failed to parse keys JSON string");
				return;
			}
		} else if (argv.file) {
			try {
				const path = process.cwd() + "/" + argv.file;
				if (!fs.existsSync(path)) {
					console.log("File does not exist:", path);
					return;
				}
				const data = fs.readFileSync(path, "utf-8");
				keyData = data;
			} catch (error) {
				console.error("Failed to read or parse keys from file");
				return;
			}
		}
		//TODO actually save keypair to location
		//await saveKeyPair(keyData)
		console.log("Key data imported successfully:", keyData);
	}
};

// TODO - Connected with Node Identification
// What is Node identity? Is it a wallet? Which wallet?
// Another auth method? Where and how should it be done and on which service
// For browser login -> listener - if auth / login confirmed
const login = {
	command: "login",
	describe: "Support for browser login",
	handler: async () => {
		console.log("Please authorize CORE CLI on at least one of your accounts");
		await open("https://testnet.mynearwallet.com/");
	}
};

// TODO - Connected with Node discovery
// Get from service / network / protocol list of available nodes
const showNodes = {
	command: "get-nodes",
	describe: "List all nodes",
	handler: () => {
		console.log("Listing all nodes...");
	}
};

// TODO - Connected with Node discovery / App discovery
// Get from service / network / protocol list of available application
// get instructions how to connect, whats needed, how does config for these apps look like
const showApps = {
	command: "get-apps",
	describe: "List all apps",
	handler: () => {
		console.log("Listing all apps...");
	}
};

yargs(hideBin(process.argv))
	.scriptName("core-cli")
	.command(join)
	.command(startSession)
	.command(addKeyPair)
	.command(login)
	.command(showNodes)
	.command(showApps)
	.help()
	.parse();
