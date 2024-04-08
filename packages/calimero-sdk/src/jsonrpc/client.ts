import {
    JsonRpcVersion,
    JsonRpcRequestId,
    RpcResponseError,
    RpcClient,
    RpcCallResponse,
    RpcCallMutResponse,
    RpcCallRequestParams,
    RpcCallMutRequestParams,
    RequestConfig
} from "../rpc";
import axios, { AxiosInstance } from "axios";

interface JsonRpcRequest<Params> {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    method: string;
    params: Params;
}

interface JsonRpcResponse<Result> {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    result?: Result;
    error?: RpcResponseError;
}

export class JsonRpcClient implements RpcClient {
    readonly path: string;
    readonly axiosInstance: AxiosInstance;

    public constructor(baseUrl: string, path: string, defaultTimeout: number = 1000) {
        this.path = path;
        this.axiosInstance = axios.create({
            baseURL: baseUrl,
            timeout: defaultTimeout,
        });
    }

    public async call<Args, Out>(params: RpcCallRequestParams<Args>, config?: RequestConfig): Promise<RpcCallResponse<Out>> {
        return await this.request<RpcCallRequestParams<Args>, RpcCallResponse<Out>>('call', params, config);
    }

    public async callMut<Args, Out>(params: RpcCallMutRequestParams<Args>, config?: RequestConfig): Promise<RpcCallMutResponse<Out>> {
        return await this.request<RpcCallMutRequestParams<Args>, RpcCallMutResponse<Out>>('call_mut', params, config);
    }

    async request<Params, Result>(method: string, params: Params, config?: RequestConfig): Promise<Result> {
        const data: JsonRpcRequest<Params> = {
            jsonrpc: '2.0',
            id: 1,
            method,
            params,
        };

        try {
            const response = await this.axiosInstance.post<JsonRpcResponse<Result>>(this.path, data, config);
            if (response.status === 200) {
                if (response.data.error) {
                    throw new Error("JSON RPC server returned error: " + response.data.error);
                }
                return response.data.result;
            } else {
                throw new Error("JSON RPC server returned error HTTP code: " + response.status);
            }
        } catch (error: any) {
            throw new Error("Error occurred during JSON RPC request: " + error.message);
        }
    }
}
