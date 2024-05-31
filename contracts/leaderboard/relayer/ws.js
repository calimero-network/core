
// Listen for messages
const WebSocket = require('ws');

module.exports = class GameEventListener {
  constructor(nodeUrl, applicationId) {
    this.ws = new WebSocket(`${nodeUrl}/ws`);
    this.ws.on('open', () => {
      const request = {
        id: this.getRandomRequestId(),
        method: 'subscribe',
        params: {
          applicationIds: [applicationId],
        },
      };
      this.ws.send(JSON.stringify(request));
    });

    this.events = {};
    this.ws.on('message', async (event) => {
      const utf8Decoder = new TextDecoder('UTF-8');
      const data = utf8Decoder.decode(event);
      await this.parseMessage(data);
    });
  }

  on(event, func) {
    this.events[event] = func;
  }

  parseMessage(msg) {
    try {
      const event = JSON.parse(msg);
      for (const e of event.result.data.events) {
        if (e.kind in this.events) {
          let bytes = new Int8Array(e.data);
          let str = new TextDecoder().decode(bytes);
          this.events[e.kind](JSON.parse(str));
        }
      }
    } catch (e) {
      console.error(`Failed to parse the json: ${e}`);
    }
  }

  getRandomRequestId() {
    return Math.floor(Math.random() * Math.pow(2, 32));
  }
};

