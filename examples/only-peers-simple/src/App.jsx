import React from "react";
import {
  JsonRpcClient,
  WsSubscriptionsClient,
} from "@calimero-is-near/calimero-p2p-sdk/lib";
import { config } from "./calimeroConfig.js";
import LoginSelector from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/LoginSelector";

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

      //TODO define how to pass this values in demo
      // const headers = {
      //   wallet_type: WalletType.NEAR,
      //   signing_key: "signing_key",
      //   signature: "signatureBase58",
      //   challenge: "contentBase58",
      // };

      // const configR = {
      //   headers,
      // };

      const resp = await client.mutate(
        {
          applicationId: config.applicationId,
          method: "create_post",
          argsJson: {
            title: "Your Post Title",
            content: "Your Post Content",
          },
        }
        // config
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
        <div style={{
          display: "flex",
          width: "100%",
          justifyContent: "center",
          alignItems: "center",
        }}>
          <LoginSelector
            applicationId="9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP"
            rpcBaseUrl="http://localhost:2428"
            successRedirect={() => console.log("success")}
            network="testnet"
            />
        </div>
      </div>
    );
  }
}

export default App;
