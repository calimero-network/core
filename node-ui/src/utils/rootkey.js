import { getWalletCallbackUrl } from "./wallet";
import axios from "axios";
import { PUBLIC_KEY, ROOT_KEY } from "./constans";

export const getParams = (location) => {
  const queryParams = new URLSearchParams(location.hash.substring(1));
  const accountId = queryParams.get("accountId");
  const signature = queryParams.get("signature");
  const publicKey = queryParams.get("publicKey");
  const callbackUrl = getWalletCallbackUrl();
  return { accountId, signature, publicKey, callbackUrl };
};

export const submitRootKeyRequest = async (params) => {
  try {
    const response = await axios.post("/admin-api/root-key", params);
    const data = response.data;
    console.log("Response received:", data);
    localStorage.setItem(ROOT_KEY, true);
    localStorage.setItem(PUBLIC_KEY, params.publicKey);
    return data;
  } catch (e) {
    console.error("Failed to submit root key request:", e);
    return false;
  }
};

export const isRootKeyAdded = () => {
  return localStorage.getItem(ROOT_KEY);
};

export const getPublicKey = () => {
  return localStorage.getItem(PUBLIC_KEY) ?? "";
};
