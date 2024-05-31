const { WebSocketServer } = require('ws');
const prompt = require('prompt-sync')({ sigint: true });

const wss = new WebSocketServer({ port: 8080 });

console.log('Waiting for a new connection...');

wss.on('connection', function connection(ws) {
  let keepAsking = true;
  ws.on('error', () => keepAsking = false);

  ws.on('message', function message(data) {
    console.log('received: %s', data);
  });

  while (keepAsking) {
    console.log('-'.repeat(process.stdout.columns));
    const action = prompt('What to do? add-score/get-score: ');
    if (action === 'add-score') {
      const account = prompt('Account: ');
      const app = prompt('Application: ');
      const score = parseInt(prompt('score: '));
      ws.send(JSON.stringify({
        action: 'add-score',
        app,
        account,
        score
      }));
    } else if (action === 'get-score') {
      const account = prompt('Account: ');
      const app = prompt('Application: ');
      ws.send(JSON.stringify({
        action: 'get-score',
        app,
        account,
      }));
    } else {
      ws.close();
    }
  }
});
