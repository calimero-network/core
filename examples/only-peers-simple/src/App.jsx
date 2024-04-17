import React from "react";
import {
  JsonRpcClient,
  WsSubscriptionsClient,
} from "@calimero-is-near/calimero-p2p-sdk/lib";
import { config } from "./calimeroConfig.js";
import { AxiosHeaders } from "axios";

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
    this.setState((prevState) => ({
      events: [...prevState.events, JSON.stringify(e)],
    }));
  };

  subscribe = async () => {
    try {
      const { subscriptionManager } = this.state;
      await subscriptionManager.connect();
      subscriptionManager.addCallback(this.eventHandler);
      subscriptionManager.subscribe([config.applicationId]);
    } catch (error) {
      console.log(error);
    }
  };

  executeRpcRequest = async () => {
    try {
      const { client } = this.state;

      const headers = new AxiosHeaders();
      headers.set("wallet_type", "NEAR");
      headers.set(
        "signing_key",
        "4XTTMDeZuuUeMrpHtuM1HWCeKJvy8hVXTyChXH65SNrGb14MD"
      );
      headers.set(
        "signature",
        "3iuv5WDUFxNNr6iuzTNaHgJxaZt3eH6UhfWQ1KXdpZLoHNdg1Xm8GNytwVYwedzACBaRcgkS8mpGvcNZMzGuhCZc"
      );
      headers.set("challenge", "HwSzpf3ieReW9ecy4D3HFJMZK8sYjrdnmXjZYCFVCJpT");

      console.log("headers", headers);

      const configR = {
        headers,
      };

      const resp = await client.mutate(
        {
          applicationId: config.applicationId,
          method: "create_post",
          argsJson: {
            title: "Your Post Title",
            content: "Your Post Content",
          },
        },
        configR
      );
      console.log("resp", resp);
      this.setState({ response: JSON.stringify(resp) });
    } catch (error) {
      console.log(error);
    }
  };

  onLoad = async () => {
    const client = new JsonRpcClient(config.nodeServerUrl, config.jsonrpcPath);
    const subscriptionManager = new WsSubscriptionsClient(
      config.nodeServerUrl,
      config.wsPath
    );
    this.setState({ client, subscriptionManager });
  };

  onDrop = async () => {
    const { client } = this.state;
    if (client) {
      await client.disconnect();
    }
  };

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
