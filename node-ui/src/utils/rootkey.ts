import { Location } from 'react-router-dom';
import { getWalletCallbackUrl } from "./wallet";
import axios from "axios";

interface UrlParams {
  accountId: string;
  signature: string;
  publicKey: string;
  callbackUrl: string;
}

interface submitRootKeyResponse {
  data?: string;
  error?: string;
}

export const getParams = (location: Location): UrlParams => {
  const queryParams = new URLSearchParams(location.hash.substring(1));
  const accountId = queryParams.get("accountId");
  const signature = queryParams.get("signature");
  const publicKey = queryParams.get("publicKey");
  const callbackUrl = getWalletCallbackUrl();
  return { accountId, signature, publicKey, callbackUrl };
};

export const submitRootKeyRequest = async (params: UrlParams): Promise<submitRootKeyResponse> => {
  try {
    const response = await axios.post("/admin-api/root-key", params);
    const message = response.data;
    return { data: message };
  } catch (error) {
    console.error("Failed to submit root key request:", error);
    return { error: error.message };
  }
};
