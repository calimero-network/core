import { Location } from 'react-router-dom';
import axios from 'axios';
import { getWalletCallbackUrl } from './wallet';
import {
  ApiRootKey,
  DidResponse,
  ETHRootKey,
  IcpRootKey,
  NearRootKey,
  Network,
  StarknetRootKey,
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
    case 280:
      return Network.ZKSYNC;
    default:
      return Network.ETH;
  }
};

export interface RootKeyObject {
  type: Network | String;
  createdAt: number;
  publicKey: string;
}

export function mapApiResponseToObjects(
  didResponse: DidResponse,
): RootKeyObject[] {
  if (didResponse?.did?.root_keys) {
    const rootKeys: (
      | ETHRootKey
      | NearRootKey
      | StarknetRootKey
      | IcpRootKey
    )[] = didResponse?.did?.root_keys?.map((obj: ApiRootKey) => {
      switch (obj.wallet.type) {
        case Network.NEAR:
          return {
            signingKey: obj.signing_key,
            createdAt: obj.created_at,
            type: Network.NEAR,
          } as NearRootKey;

        case Network.ETH:
          return {
            signingKey: obj.signing_key,
            type: Network.ETH,
            createdAt: obj.created_at,
            chainId: obj.wallet.chainId ?? 1,
          } as ETHRootKey;

        case Network.ICP:
          return {
            signingKey: obj.signing_key,
            type: Network.ICP,
            createdAt: obj.created_at,
          } as IcpRootKey;

        case Network.STARKNET:
        default:
          return {
            signingKey: obj.signing_key,
            type: Network.STARKNET + ' ' + obj.wallet.walletName,
            createdAt: obj.created_at,
          } as StarknetRootKey;
      }
    });

    return rootKeys.map((item) => ({
      type:
        item.type === Network.ETH
          ? getMetamaskType((item as ETHRootKey).chainId ?? 1)
          : item.type,
      createdAt: item.createdAt,
      publicKey:
        item.type === 'NEAR'
          ? (item as NearRootKey).signingKey.split(':')[1]!.trim()
          : item.signingKey,
    }));
  } else {
    return [];
  }
}
