import { Location } from 'react-router-dom';
import axios from 'axios';
import { getWalletCallbackUrl } from './wallet';
import {
  ApiRootKey,
  DidResponse,
  ETHRootKey,
  NearRootKey,
  Network,
} from '../api/dataSource/NodeDataSource';
import { getAppEndpointKey } from './storage';

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
  const accountId = queryParams.get('accountId') ?? '';
  const signature = queryParams.get('signature') ?? '';
  const publicKey = queryParams.get('publicKey') ?? '';
  const callbackUrl = getWalletCallbackUrl();
  return { accountId, signature, publicKey, callbackUrl };
};

export const submitRootKeyRequest = async (
  params: UrlParams,
): Promise<submitRootKeyResponse> => {
  try {
    const response = await axios.post(
      `${getAppEndpointKey()}/admin-api/root-key`,
      params,
    );
    const message = response.data;
    return { data: message };
  } catch (error) {
    console.error('Failed to submit root key request:', error);
    // TODO add error types
    // @ts-ignore: Property 'message' does not exist on type 'unknown'
    return { error: error.message };
  }
};

const getMetamaskType = (chainId: number): Network => {
  switch (chainId) {
    case 1:
      return Network.ETH;
    case 56:
      return Network.BNB;
    case 42161:
      return Network.ARB;
    case 324:
      return Network.ZK;
    default:
      return Network.ETH;
  }
};

export interface RootKeyObject {
  type: Network;
  createdAt: number;
  publicKey: string;
}

export function mapApiResponseToObjects(
  didResponse: DidResponse,
): RootKeyObject[] {
  if (didResponse?.did?.root_keys) {
    const rootKeys: (ETHRootKey | NearRootKey)[] =
      didResponse?.did?.root_keys?.map((obj: ApiRootKey) => {
        if (obj.wallet.type === Network.NEAR) {
          const nearObject: NearRootKey = {
            signingKey: obj.signing_key,
            createdAt: obj.created_at,
            type: Network.NEAR,
          };
          return nearObject;
        } else {
          const ethObject: ETHRootKey = {
            signingKey: obj.signing_key,
            type: Network.ETH,
            createdAt: obj.created_at,
            chainId: obj.wallet.chainId ?? 1,
          };
          return ethObject;
        }
      });
    return rootKeys.map((item) => ({
      type:
        item.type === Network.NEAR
          ? Network.NEAR
          : getMetamaskType(item.chainId ?? 1),
      createdAt: item.createdAt,
      publicKey:
        item.type === 'NEAR'
          ? item.signingKey.split(':')[1]!.trim()
          : item.signingKey,
    }));
  } else {
    return [];
  }
}
