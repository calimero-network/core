import { HttpTransport, JsonRpcClient } from 'calimero-p2p-sdk'

function App() {
  const transport = new HttpTransport("http://localhost:2529", "/jsonrpc");
  const rpcClient = new JsonRpcClient(transport);

  const executeRpcRequest = async () => {
    try {
      const response = await rpcClient.request("call_muts", {
        applicationId: "/calimero/experimental/app/9GBJQtDr2XcAhipZT8F5ZD677QTuXavX1PHoBCH68RGv",
        method: "create_post",
        argsJson: {
          title: "Your Post Title",
          content: "Your Post Content"
        }
      });
      console.log(response.data);
    } catch (error) {
      console.log(error);
    }

  }

  executeRpcRequest();
  return <div></div>;

}

export default App
