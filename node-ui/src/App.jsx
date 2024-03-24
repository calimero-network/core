import './App.css'
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { Buffer } from 'buffer';

const nonce = Buffer.alloc(32, 'hardcoded-string-for-testing-only');

const verifyOwner = async () => {
  const selector = await setupWalletSelector({
    network: "testnet",
    modules: [setupMyNearWallet()],
  });
  const wallet = await selector.wallet("my-near-wallet");
  await wallet.signMessage({ message: "helloworld", recipient: "me", nonce, callbackUrl: window.location.href });
}

function App() {
  return (
    <>

      <h1>Calimero node admin page</h1>
      <div className="card">
        <button onClick={() => verifyOwner()}>
          Login with Near
        </button>
        </div>
    </>
  )
}

export default App
