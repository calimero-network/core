import React from "react";
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { Buffer } from "buffer";
import axios from "axios";
import { Login } from "../components/login/Login";
import { Footer } from "../components/footer/Footer";
import styled from "styled-components";
import { getWalletCallbackUrl } from "../utils/wallet";

const fetchChallenge = async () => {
  const body = {
    applicationId: "admin-ui",
  };
  const response = await axios.post("/admin-api/request-challenge", body);
  const payload = response.data.data;
  console.log("Challenge received:", payload);
  return payload;
};

const verifyOwner = async () => {
  let nonceBase64 = null;
  let challengeObject = null;
  try {
    challengeObject = await fetchChallenge();
  } catch (e) {
    console.error("Failed to fetch challenge:", e);
    return;
  }
  const nonce = Buffer.from(challengeObject.nonce, "base64");

  const selector = await setupWalletSelector({
    network: "testnet",
    modules: [setupMyNearWallet()],
  });
  const wallet = await selector.wallet("my-near-wallet");
  const callbackUrl = getWalletCallbackUrl();
  const message = "helloworld";
  const recipient = "me";
  console.log("Signing message:", {
    message,
    recipient,
    nonceBase64,
    callbackUrl,
  });
  await wallet.signMessage({ message, nonce, recipient, callbackUrl });
};

const BootstrapWrapper = styled.div`
  height: 150px;
`;

function Bootstrap() {
  return (
    <BootstrapWrapper>
      <Login verifyOwner={verifyOwner} />
      <Footer />
    </BootstrapWrapper>
  );
}

export default Bootstrap;
