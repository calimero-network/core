import { Buffer } from "buffer";
import * as nearAPI from "near-api-js";

const JSON_RPC_ENDPOINT = "https://rpc.testnet.near.org";

export function useRPC() {
  const getPackages = async () => {
    const provider = new nearAPI.providers.JsonRpcProvider(JSON_RPC_ENDPOINT);

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

    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getPackage = async (id) => {
    const provider = new nearAPI.providers.JsonRpcProvider(JSON_RPC_ENDPOINT);

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

    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getReleases = async (id) => {
    const provider = new nearAPI.providers.JsonRpcProvider(JSON_RPC_ENDPOINT);

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

    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getLatestRelease = async (id) => {
    const provider = new nearAPI.providers.JsonRpcProvider(JSON_RPC_ENDPOINT);

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
    const releases = JSON.parse(Buffer.from(rawResult.result).toString());
    if (releases.length === 0) {
      return null;
    }
    return releases[releases.length - 1];
  };

  return { getPackages, getReleases, getPackage, getLatestRelease };
}