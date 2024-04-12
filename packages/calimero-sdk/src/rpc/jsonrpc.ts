import {
    RpcResponseError,
    RpcClient,
    RpcQueryResponse,
    RpcMutateResponse,
    RpcQueryParams,
    RpcMutateParams,
    RequestConfig
} from "../rpc";
import axios, { AxiosInstance } from "axios";

type JsonRpcVersion = '2.0'
type JsonRpcRequestId = string | number;

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

    public async query<Args, Output>(params: RpcQueryParams<Args>, config?: RequestConfig): Promise<RpcQueryResponse<Output>> {
        return await this.request<RpcQueryParams<Args>, RpcQueryResponse<Output>>('query', params, config);
    }

    public async mutate<Args, Output>(params: RpcMutateParams<Args>, config?: RequestConfig): Promise<RpcMutateResponse<Output>> {
        return await this.request<RpcMutateParams<Args>, RpcMutateResponse<Output>>('mutate', params, config);
    }

    async request<Params, Result>(method: string, params: Params, config?: RequestConfig): Promise<Result> {
        const requestId = this.getRandomRequestId()
        const data: JsonRpcRequest<Params> = {
            jsonrpc: '2.0',
            id: requestId,
            method,
            params,
        };

        try {
            const response = await this.axiosInstance.post<JsonRpcResponse<Result>>(this.path, data, config);
            if (response.status === 200) {
                if (response.data.error) {
                    throw new Error("JSON RPC server returned error: " + response.data.error);
                }
                if (response.data.id !== requestId) {
                    throw new Error(`JSON RPC server returned response with invalid ID, expected: ${requestId} got: ${response.data.id}`);
                }
                return response.data.result;
            } else {
                throw new Error(`JSON RPC server returned error HTTP code: ${response.status}`);
            }
        } catch (error: any) {
            throw new Error(`Error occurred during JSON RPC request: ${JSON.stringify(error.message)}`);
        }
    }

    getRandomRequestId(): number {
        return Math.floor(Math.random() * Math.pow(2, 32));
    }
}
