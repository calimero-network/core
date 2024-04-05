import { JsonRpcClient } from 'calimero-p2p-sdk'

function App() {
  const client = new JsonRpcClient('http://localhost:2529', '/jsonrpc');

  const executeRpcRequest = async () => {
    try {
      const resp = await client.callMut(
        "/calimero/experimental/app/FyweziaTzQAahZmdZ3kjUwFr52RCKQYqcpiPDXCNMNzN",
        "create_post",
        {
          title: "Your Post Title",
          content: "Your Post Content"
        }
      );
      console.log(resp);
    } catch (error) {
      console.log(error);
    }
  }

  executeRpcRequest();
  return <div></div>;
}

export default App
