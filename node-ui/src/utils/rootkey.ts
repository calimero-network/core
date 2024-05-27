import { Location } from 'react-router-dom';
import axios from "axios";
import { getWalletCallbackUrl } from "./wallet";
import { RootKey } from "../api/dataSource/NodeDataSource";

export interface UrlParams {
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
  const accountId = queryParams.get("accountId") ?? "";
  const signature = queryParams.get("signature") ?? "";
  const publicKey = queryParams.get("publicKey") ?? "";
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
    // TODO add error types
    // @ts-ignore: Property 'message' does not exist on type 'unknown'
    return { error: error.message };
  }
};

export interface RootKeyObject {
  type: string;
  date: number;
  publicKey: string;
}

export function mapApiResponseToObjects(didList: RootKey[]): RootKeyObject[] {
  return didList.map((item) => ({
      type: item.walletType,
      date: item.date,
      publicKey: item.walletType === "NEAR" ? item.signingKey.split(":")[1]!.trim() : item.signingKey,
    }));
}
