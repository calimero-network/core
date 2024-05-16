const nearAPI = require('near-api-js');
const fs = require('fs');
const commandLineArgs = require('command-line-args');
const WebSocket = require('ws');

const { Contract } = nearAPI;

const createKeyStore = async () => {
  const { KeyPair, keyStores } = nearAPI;

  const ACCOUNT_ID = 'huge-drop.testnet';
  const NETWORK_ID = 'testnet';
  const KEY_PATH = '/home/saeed/.near-credentials/testnet/huge-drop.testnet.json';

  const credentials = JSON.parse(fs.readFileSync(KEY_PATH));
  const myKeyStore = new keyStores.InMemoryKeyStore();
  myKeyStore.setKey(
    NETWORK_ID,
    ACCOUNT_ID,
    KeyPair.fromString(credentials.private_key)
  );

  return myKeyStore;
};

let keyStore;
const connectToNear = async () => {
  keyStore = await createKeyStore();
  const connectionConfig = {
    networkId: 'testnet',
    keyStore,
    nodeUrl: 'https://rpc.testnet.near.org',
    walletUrl: 'https://testnet.mynearwallet.com/',
    helperUrl: 'https://helper.testnet.near.org',
    explorerUrl: 'https://testnet.nearblocks.io',
  };
  const { connect } = nearAPI;
  const nearConnection = await connect(connectionConfig);
  return nearConnection;
};


const addScore = async (account_id, app_name, score) => {
  if (contract === null) {
    throw new Error('Contract is not initialized');
  }

  const account = await near.account('huge-drop.testnet');
  await contract.add_score({
    signerAccount: account,
    args: {
      app_name,
      account_id,
      score
    },
  }
  );
};

const getScore = async (account_id, app_name) => {
  if (contract === null) {
    throw new Error('Contract is not initialized');
  }

  return await contract.get_score({
    app_name,
    account_id,
  },
  );
};

let contract = null;
let near = null;

async function main() {
  const optionDefinitions = [
    { name: 'subscribe', type: Boolean },
    { name: 'add-score', type: Boolean },
    { name: 'get-score', type: Boolean },
    { name: 'account', type: String },
    { name: 'score', type: Number },
    { name: 'app', type: String }
  ];

  const options = commandLineArgs(optionDefinitions);

  const nearConnection = await connectToNear();
  near = nearConnection;
  contract = new Contract(
    nearConnection.connection,
    'huge-drop.testnet',
    {
      changeMethods: ['add_score'],
      viewMethods: ['get_version', 'get_score']
    }
  );
  if (options.subscribe) {
    subscribe();
  }
  else if (options['add-score']) {
    const { account, app, score } = options;
    await addScore(account, app, score);
    console.log(`Score added for account: ${account}, app: ${app}, score: ${score}`);
  } else if (options['get-score']) {
    const { account, app } = options;
    const score = await getScore(account, app);
    console.log(`${account} score is: ${score}`);
  }
}

const subscribe = () => {
  const ws = new WebSocket('ws://localhost:8080');
  ws.on('error', console.error);

  ws.on('open', function open() {
    console.log('open');
  });

  ws.on('message', async function message(data) {
    const message = JSON.parse(data);
    const { action } = message;
    console.log(`Receive ${action} from the server.`);
    if (action === 'add-score') {
      const { app, account, score } = message;
      await addScore(account, app, score);
      console.log(`Score added. Account: ${account}, App: ${app}, Score: ${score}`);
    } else if (action === 'get-score') {
      const { app, account } = message;
      const score = await getScore(account, app);
      console.log(`${account} score is: ${score}`);
    }
  });
};

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});