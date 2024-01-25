#!/usr/bin/env node
import yargs, { Argv } from "yargs";
import { hideBin } from "yargs/helpers";
import { JoinCommandOptions, StartSessionCommandOptions } from "./types/index.js";
import * as fs from "fs";
import open from "open";
import ora, {Color} from "ora";
import inquirer, { Answers } from "inquirer";
import * as progress from "cli-progress";
import chalk from "chalk";
import Table from "cli-table";
// TODO - update output with right chalk colors

//TOOD - does user connect to network or does he just reveals his IP and Port in Node Discovery?
// After "Join" process is finished the handler is considered complete.
// join arguments (addr, port) refer to known peer or boostrap server in the network
// *add more arguments if needed
const join = {
	command: "join",
	describe: "join P2P network on specific address",
	builder: (yargs: Argv) => {
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
		const spinner = ora("Joining the network").start();

		const updateSpinner = async (text: string, color: Color, delay: number) => {
			await new Promise((resolve) => setTimeout(resolve, delay));
			spinner.color = color;
			spinner.text = text;
		};

		await updateSpinner("Spinning up your node...", "yellow", 1000);
		await updateSpinner("Waiting response from server...", "yellow", 1000);
		await updateSpinner("Adding your node to network discovery", "blue", 1000);
		await updateSpinner("Starting connection...", "blue", 1000);
		spinner.succeed("Connected successfully");
		console.log("Node discoverable at address:", argv.address);
	}
};

//TODO - Period during which user interacts with the application
// Suppose in P2P Chat - users are connected and that is one Application sessions
// Config file - TODO what is it, is it needed, ...
const startSession = {
	command: "start-session",
	describe: "Start an Application Session",
	builder: (yargs: Argv) => {
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

		const progressBar = new progress.SingleBar({
			format: "Connecting [{bar}] {percentage}% | ETA: {eta}s | {value}/{total} Steps",
			stopOnComplete: true,
			barsize: 30,
		  }, progress.Presets.shades_classic);
		  const totalSteps = 10;
		  const delay = 1000;
		  async function simulateProgress() {
			progressBar.start(totalSteps, 0);
			for (let step = 0; step < totalSteps; step++) {
			  await new Promise((resolve) => setTimeout(resolve, delay));
			  progressBar.update(step + 1);
			}
			progressBar.stop();
			console.log(chalk.green("✔ Connected successfully"));
		  }
		  simulateProgress();
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
	builder: {},
	handler: async () => {
		const answers: Answers = await inquirer.prompt([
			{
				type: "list",
				name: "method",
				message: "How would you like to import the key pair?",
				choices: ["Import from JSON string", "Import from JSON file"],
			  },
			  {
				type: "input",
				name: "id",
				message: "Enter your id:",
				when: (answers: { method: string }) => answers.method === "Import from JSON string",
			  },
			  {
				type: "input",
				name: "pubKey",
				message: "Enter your pubKey:",
				when: (answers: { method: string }) => answers.method === "Import from JSON string",
			  },
			  {
				type: "input",
				name: "privKey",
				message: "Enter your privKey:",
				when: (answers: { method: string }) => answers.method === "Import from JSON string",
			  },
			  {
				type: "input",
				name: "filePath",
				message: "Enter the file path:",
				when: (answers: { method: string }) => answers.method === "Import from JSON file",
			  },
		]);
		if (answers.method === "Import from JSON string") {
			//save to storage
		} else if (answers.filePath){
			try {
				const path = process.cwd() + "/" + answers.filePath;
				if (!fs.existsSync(path)) {
						console.log("File does not exist:", path);
						return;
				}
				const data = fs.readFileSync(path, "utf-8");
				console.log(data);
			} catch (error) {
					console.error("Failed to read or parse keys from file");
					return;
			}
		}
		console.log("Key pair saved to storage.")
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
		const answers: Answers = await inquirer.prompt([
			{
				type: "list",
				name: "method",
				message: "How would you link to login?",
				choices: ["CLI login", "Browser Login"],
			  },
			  {
				type: "input",
				name: "id",
				message: "Enter your id:",
				when: (answers: { method: string }) => answers.method === "CLI login",
			  },
			  {
				type: "input",
				name: "privKey",
				message: "Enter your private Key:",
				when: (answers: { method: string }) => answers.method === "CLI login",
			  },
		]);
		if (answers.method === "CLI login") {
			console.log(chalk.green("Successfully logged in!"));
		} else {
			console.log(chalk.yellow("Please authorize CORE CLI on at least one of your accounts"));
			await open("https://testnet.mynearwallet.com/");
		}
	}
};

// TODO - Connected with Node discovery
// Get from service / network / protocol list of available nodes
const showNodes = {
	command: "get-nodes",
	describe: "List all nodes",
	handler: () => {
		console.log(chalk.green("Listing all nodes..."));
		const table = new Table({
			head: ["Node", "IP Address", "Configuration"],
		  });
		table.push(["q2edmwslq4w", "127.23.12.3", "P2P"]);
		table.push(["gkelsm24ls13s", "94.43.123.2", "P2P"]);

		console.log(table.toString());
	}
};

// TODO - Connected with Node discovery / App discovery
// Get from service / network / protocol list of available application
// get instructions how to connect, whats needed, how does config for these apps look like
const showApps = {
	command: "get-apps",
	describe: "List all apps",
	handler: () => {
		console.log(chalk.green("Listing all apps..."));
		const table = new Table({
			head: ["Application", "IP Address", "Configuration"],
		  });
		table.push(["P2P Chat", "123.34.21.4:5314", "Node ID, Metadata"]);
		table.push(["P2P Docs", "143.32.1.89:1249", "Node ID, Metadata"]);
		console.log(table.toString());
	}
};

yargs(hideBin(process.argv))
	.demandCommand(1, "You must specify a command. Use --help flag to list available commands")
	.scriptName("core-cli")
	.command(join)
	.command(startSession)
	.command(addKeyPair)
	.command(login)
	.command(showNodes)
	.command(showApps)
	.version()
	.help()
	.parse();
