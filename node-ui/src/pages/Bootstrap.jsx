import React from 'react';
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { Buffer } from 'buffer';
import axios from 'axios';

const fetchChallenge = async() => {
    const response = await axios.post("/admin-api/request-challenge");
    const challenge = response.data;
    console.log('Challenge received:', challenge);
    return challenge;
}

const verifyOwner = async () => {
    let nonceBase64 = null;
    try {
        nonceBase64 = await fetchChallenge();
    } catch (e) {
      console.error('Failed to fetch challenge:', e);
      return;
    }
    const nonce = Buffer.from(nonceBase64, 'base64');
  
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    await wallet.signMessage({ message: "helloworld", recipient: "me", nonce, callbackUrl: window.location.href + "confirm-wallet" });
  }


function Bootstrap() {
  return (
    <>
      <h1>Calimero node admin page</h1>
      <h1>Select your wallet to start using calimero</h1>
      <div className="card">
        <button onClick={() => verifyOwner()}>
          Login with Near
        </button>
        </div>
    </>
  )
}

export default Bootstrap;
