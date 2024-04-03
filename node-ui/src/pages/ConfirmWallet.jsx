import React from "react";
import { useLocation } from "react-router-dom";
import axios from "axios";
import { getWalletCallbackUrl } from "../utils/wallet";

export default function ConfirmWallet() {
  const location = useLocation();
  const params = getParams(location);
  return (
    <>
      <h1>Confirm Wallet</h1>
      <p>Account ID: {params.accountId}</p>
      <p>Signature: {params.signature}</p>
      <p>Public Key: {params.publicKey}</p>
      <p>Callback Url: {params.callbackUrl}</p>
      <button onClick={() => submitRootKeyRequest(params)}>Submit</button>
    </>
  );
}

const getParams = (location) => {
  const queryParams = new URLSearchParams(location.hash.substring(1)); // skip the leading '#'
  const accountId = queryParams.get("accountId");
  const signature = queryParams.get("signature");
  const publicKey = queryParams.get("publicKey");
  const callbackUrl = getWalletCallbackUrl();
  return { accountId, signature, publicKey, callbackUrl };
};

const submitRootKeyRequest = async (params) => {
  const response = await axios.post("/admin-api/root-key", params);
  const data = response.data;
  console.log("Response received:", data);
  return data;
};
