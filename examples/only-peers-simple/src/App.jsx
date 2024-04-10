import React from 'react';
import { JsonRpcClient, WsSubscriptionManager } from 'calimero-p2p-sdk';
import { config } from './calimeroConfig.js';

class App extends React.Component {
  constructor(props) {
    super(props);
    this.state = {
      client: null,
      subscriptionManager: null,
      response: null,
      events: [],
    };
  }

  eventHandler = async (e) => {
    this.setState(prevState => ({ events: [...prevState.events, JSON.stringify(e)] }));
  }

  subscribe = async () => {
    try {
      const { subscriptionManager } = this.state;
      await subscriptionManager.connect();
      subscriptionManager.addCallback(this.eventHandler);
      subscriptionManager.subscribe([config.applicationId]);
    } catch (error) {
      console.log(error);
    }
  }

  executeRpcRequest = async () => {
    try {
      const { client } = this.state;
      const resp = await client.mutate({
        applicationId: config.applicationId,
        method: "create_post",
        argsJson: {
          title: "Your Post Title",
          content: "Your Post Content"
        }
      });
      this.setState({ response: JSON.stringify(resp) });
    } catch (error) {
      console.log(error);
    }
  }

  onLoad = async () => {
    const client = new JsonRpcClient(config.nodeServerUrl, config.jsonrpcPath);
    const subscriptionManager = new WsSubscriptionManager(config.nodeServerUrl, config.wsPath);
    this.setState({ client, subscriptionManager });
  }

  onDrop = async () => {
    const { client } = this.state;
    if (client) {
      await client.disconnect();
    }
  }

  componentDidMount() {
    this.onLoad();
  }

  componentWillUnmount() {
    this.onDrop();
  }

  render() {
    const { response, events } = this.state;
    return (
      <div>
        <button onClick={this.executeRpcRequest}>Execute RPC Request</button>
        <p>{response}</p>
        <button onClick={this.subscribe}>Subscribe</button>
        <table>
          <thead>
            <tr>
              <th>Event</th>
            </tr>
          </thead>
          <tbody>
            {events.map((event, index) => (
              <tr key={index}>
                <td>{event}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  }
}

export default App;
