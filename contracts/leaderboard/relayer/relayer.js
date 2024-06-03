const nearAPI = require('near-api-js');
const fs = require('fs');
const commandLineArgs = require('command-line-args');
const GameEventListener = require('./ws');

const { Contract } = nearAPI;

const createKeyStore = async () => {
  const { KeyPair, keyStores } = nearAPI;

  const ACCOUNT_ID = 'highfalutin-act.testnet';
  const NETWORK_ID = 'testnet';
  const KEY_PATH =
    '/home/saeed/.near-credentials/testnet/highfalutin-act.testnet.json';

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

  const account = await near.account('highfalutin-act.testnet');
  await contract.add_score({
    signerAccount: account,
    args: {
      app_name,
      account_id,
      score,
    },
  });
};

const getScore = async (account_id, app_name) => {
  if (contract === null) {
    throw new Error('Contract is not initialized');
  }

  return await contract.get_score({
    app_name,
    account_id,
  });
};

const getScores = async (app_name) => {
  if (contract === null) {
    throw new Error('Contract is not initialized');
  }

  return await contract.get_scores({
    app_name,
  });
};

let contract = null;
let near = null;

async function main() {
  const optionDefinitions = [
    { name: 'subscribe', type: Boolean },
    { name: 'add-score', type: Boolean },
    { name: 'get-score', type: Boolean },
    { name: 'get-scores', type: Boolean },
    { name: 'account', type: String },
    { name: 'score', type: Number },
    { name: 'app', type: String },
    { name: 'applicationId', type: String },
    { name: 'nodeUrl', type: String },
  ];

  const options = commandLineArgs(optionDefinitions);

  const nearConnection = await connectToNear();
  near = nearConnection;
  contract = new Contract(
    nearConnection.connection,
    'highfalutin-act.testnet',
    {
      changeMethods: ['add_score'],
      viewMethods: ['get_version', 'get_score', 'get_scores'],
    }
  );
  if (options.subscribe) {
    const { applicationId, nodeUrl } = options;
    console.log(`Subscribed for the events of ${applicationId}`);
    subscribe(applicationId, nodeUrl);
  } else if (options['add-score']) {
    const { account, app, score } = options;
    await addScore(account, app, score);
    console.log(
      `Score added for account: ${account}, app: ${app}, score: ${score}`
    );
  } else if (options['get-score']) {
    const { account, app } = options;
    const score = await getScore(account, app);
    console.log(`${account} score is: ${score}`);
  } else if (options['get-scores']) {
    const { app } = options;
    const scores = await getScores(app);
    console.log(`Scores for ${app}: ${JSON.stringify(scores)}`);
  }
}

let eventListener;
let players = {};
const subscribe = (applicationId, nodeUrl) => {
  eventListener = new GameEventListener(nodeUrl, applicationId);
  eventListener.on('NewPlayer', (player) => {
    players[player.id] = player.name;
  });

  eventListener.on('GameOver', (winner) => {
    addScore(players[winner.winner], 'rsp', 1000).then(() =>
      console.log(`Score added for ${players[winner.winner]}`)
    ).catch(e => {
      console.error(`Failed to add the score. ${e}`);
    });
  });
};

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
