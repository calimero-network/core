import { JsonRpcClient, CalimeroClient } from 'calimero-p2p-sdk'

function App() {
  const rpcClient = new JsonRpcClient("http://localhost:2529", "/jsonrpc");
  const calimeroClient = new CalimeroClient(rpcClient);

  const executeRpcRequest = async () => {
    try {
      const response = await calimeroClient.callMethod(
        "/calimero/experimental/app/9GBJQtDr2XcAhipZT8F5ZD677QTuXavX1PHoBCH68RGv",
        "create_post",
        {
          title: "Your Post Title",
          content: "Your Post Content"
        }
      );
      console.log(response.data);
    } catch (error) {
      console.log(error);
    }

  }

  executeRpcRequest();
  return <div></div>;

}

export default App
