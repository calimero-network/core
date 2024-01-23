export interface JoinCommandOptions {
	address: string;
	port?: number;
	token?: string;
}

export interface StartSessionCommandOptions {
	app: string;
	config?: string;
	debug?: boolean;
}

export interface AddKeysCommandOptions {
	keys?: string;
	file?: string;
}

export interface ReadStorageCommandOptions {
	path: string;
}