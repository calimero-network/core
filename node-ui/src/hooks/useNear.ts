import { Buffer } from "buffer";
import * as nearAPI from "near-api-js";
import { Package, Release } from "../pages/Applications";

const JSON_RPC_ENDPOINT = "https://rpc.testnet.near.org";

export function useRPC() {
  const getPackages = async (): Promise<Package[]> => {
    const provider = new nearAPI.providers.JsonRpcProvider({ url: JSON_RPC_ENDPOINT });

    const rawResult = await provider.query({
      request_type: "call_function",
      account_id: "calimero-package-manager.testnet",
      method_name: "get_packages",
      args_base64: btoa(
        JSON.stringify({
          offset: 0,
          limit: 100,
        })
      ),
      finality: "final",
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getPackage = async (id: string): Promise<Package> => {
    const provider = new nearAPI.providers.JsonRpcProvider({ url: JSON_RPC_ENDPOINT });

    const rawResult = await provider.query({
      request_type: "call_function",
      account_id: "calimero-package-manager.testnet",
      method_name: "get_package",
      args_base64: btoa(
        JSON.stringify({
          id,
        })
      ),
      finality: "final",
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getReleases = async (id: string): Promise<Release[]> => {
    const provider = new nearAPI.providers.JsonRpcProvider({ url: JSON_RPC_ENDPOINT });

    const rawResult = await provider.query({
      request_type: "call_function",
      account_id: "calimero-package-manager.testnet",
      method_name: "get_releases",
      args_base64: btoa(
        JSON.stringify({
          id,
          offset: 0,
          limit: 100,
        })
      ),
      finality: "final",
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getLatestRelease = async (id: string): Promise<Release | null> => {
    const provider = new nearAPI.providers.JsonRpcProvider({ url: JSON_RPC_ENDPOINT });

    const rawResult = await provider.query({
      request_type: "call_function",
      account_id: "calimero-package-manager.testnet",
      method_name: "get_releases",
      args_base64: btoa(
        JSON.stringify({
          id,
          offset: 0,
          limit: 100,
        })
      ),
      finality: "final",
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    const releases = JSON.parse(Buffer.from(rawResult.result).toString());
    if (releases.length === 0) {
      return null;
    }
    return releases[releases.length - 1];
  };

  return { getPackages, getReleases, getPackage, getLatestRelease };
}
