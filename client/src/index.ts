#!/usr/bin/env node
import yargs from "yargs";
import { hideBin } from "yargs/helpers";
import { JoinCommandOptions, StartSessionCommandOptions, AddKeysCommandOptions } from "./types";
// eslint-disable-next-line @typescript-eslint/no-var-requires
const chalk = require("chalk");
// eslint-disable-next-line @typescript-eslint/no-var-requires
const fs = require("fs");
// eslint-disable-next-line @typescript-eslint/no-var-requires
const open = require("open");

const join = {
	command: "join",
	describe: "join P2P network",
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
	handler: (argv: JoinCommandOptions) => {
		console.log(`Joining network at address ${chalk.green(`${argv.address}:${argv.port || 3000}`)}`);
		if (argv.token) {
			console.log(`Using authentication token: ${argv.token}`);
		}
	}
};

const startSession = {
	command: "start-session",
	describe: "Start an application session",
	builder: (yargs: yargs.Argv) => {
		return yargs
			.usage("Usage: core-cli start-session --app <appName> [--config <configFilePath>] [--debug]")
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
			.option("debug", {
				alias: "d",
				describe: "Run in debug mode (optional)",
				type: "boolean"
			})
			.check((argv) => {
				if (!argv.app) {
					throw new Error("The --app option is required");
				}
				return true;
			});
	},
	handler: (argv: StartSessionCommandOptions) => {
		console.log(`Starting application session for ${chalk.green(argv.app)}`);
		if (argv.config) {
			console.log(`Using configuration file: ${argv.config}`);
		}
		if (argv.debug) {
			console.log(chalk.yellow("Debug mode is enabled"));
		}
		console.log(chalk.blue("The application session is running... (Press Ctrl+C to stop)"));
		const intervalId = setInterval(() => {
			console.log(chalk.gray("Session is active..."));
		}, 10000);
		process.on("SIGINT", () => {
			clearInterval(intervalId);
			console.log(chalk.red("Application session ended."));
			process.exit(0);
		});
	}
};

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
				console.error(chalk.red("Failed to parse keys JSON string"));
				return;
			}
		} else if (argv.file) {
			try {
				const path = process.cwd() + "/" + argv.file;
				if (!fs.existsSync(path)) {
					console.log("File does not exist:", path);
				}
				const data = fs.readFileSync(path, "utf-8");
				keyData = data;
			} catch (error) {
				console.log("🚀 ~ error:", error);
				console.error(chalk.red("Failed to read or parse keys from file"));
				return;
			}
		}
		console.log(chalk.green("Key data imported successfully:"), keyData);
	}
};

const login = {
	command: "login",
	describe: "Support for browser login",
	handler: async () => {
		console.log(chalk.yellow("Please authorize CORE CLI on at least one of your accounts"));
		await open("https://testnet.mynearwallet.com/");
	}
};

const showNodes = {
	command: "get-nodes",
	describe: "List all nodes",
	handler: () => {
		console.log(chalk.green("Listing all nodes..."));
	}
};
  
const showApps = {
	command: "get-apps",
	describe: "List all apps",
	handler: () => {
		console.log(chalk.green("Listing all apps..."));
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
