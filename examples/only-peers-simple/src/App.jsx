import { JsonRpcClient, WsSubscriptionManager } from 'calimero-p2p-sdk'
import { useEffect, useState } from 'react';
import { config } from './calimeroConfig.js'

function App() {
  const [client, setClient] = useState(null);
  const [subscriptionManager, setSubscriptionManager] = useState(null);

  const [response, setResponse] = useState(null);
  const [events, setEvents] = useState([]);

  const eventHandler = async (e) => {
    setEvents(prevEvents => [...prevEvents, JSON.stringify(e)]);
  }

  const subscribe = async () => {
    try {
      await subscriptionManager.connect();
      subscriptionManager.addCallback(eventHandler);
      subscriptionManager.subscribe([config.applicationId]);
    } catch (error) {
      console.log(error);
    }
  }

  const executeRpcRequest = async () => {
    try {
      const resp = await client.mutate(
        {
          applicationId: config.applicationId,
          method: "create_post",
          argsJson: {
            title: "Your Post Title",
            content: "Your Post Content"
          }
        }
      );
      setResponse(JSON.stringify(resp));
    } catch (error) {
      console.log(error);
    }
  }

  useEffect(
    () => {
      async function bootstrap() {
        const client = new JsonRpcClient(config.nodeServerUrl, config.jsonrpcPath);
        setClient(client);

        const subscriptionManager = new WsSubscriptionManager(config.nodeServerUrl, config.wsPath);
        setSubscriptionManager(subscriptionManager);
      };

      if (!client && !subscriptionManager) {
        bootstrap();
      }
    }, []
  );

  return <div>
    <button onClick={executeRpcRequest}>Execute RPC Request</button>
    <p>{response}</p>
    <button onClick={subscribe}>Subscribe</button>
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
  </div>;
}

export default App
