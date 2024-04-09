import { JsonRpcClient, WebSocketSubscriptionManager } from 'calimero-p2p-sdk'

function App() {
  const applicationId = "/calimero/experimental/app/5ViNGx78QzsqXQn48QjpNFYNYmWqZruSK4GGhzV5WhR"
  const client = new JsonRpcClient('http://localhost:2529', '/jsonrpc');
  const subscriptionManager = new WebSocketSubscriptionManager('ws://localhost:2529', '/ws')

  const eventHandler = async (e) => {
    console.log(`event handler: ${e}`);
  }

  const subscribe = async () => {
    try {
      await subscriptionManager.connect();
      subscriptionManager.addCallback(eventHandler);
      subscriptionManager.subscribe([applicationId]);
    } catch (error) {
      console.log(error);
    }
  }

  const executeRpcRequest = async () => {
    try {
      const resp = await client.mutate(
        {
          applicationId: applicationId,
          method: "create_post",
          argsJson: {
            title: "Your Post Title",
            content: "Your Post Content"
          }
        }
      );
      console.log(resp);
    } catch (error) {
      console.log(error);
    }
  }

  subscribe();
  executeRpcRequest();
  return <div></div>;
}

export default App
