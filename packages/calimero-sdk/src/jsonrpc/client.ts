import {
    RpcClient,
    RpcRequest,
    RpcResponse,
    RpcCallRequest,
    RpcCallResponse,
    RpcCallMutRequest,
    RpcCallMutResponse,
    RpcCallRequestParams,
    RpcCallMutRequestParams,
    RequestConfig
} from "../rpc";
import { JsonRpcRequest, JsonRpcResponse } from "./request";
import axios, { AxiosInstance } from "axios";

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

    public async call(params: RpcCallRequestParams, config: RequestConfig): Promise<RpcCallResponse> {
        const payload: RpcCallRequest = {
            method: 'call',
            params
        };

        return await this.request<RpcCallRequest, RpcCallResponse>(payload, config);
    }

    public async callMut(params: RpcCallMutRequestParams, config: RequestConfig): Promise<RpcCallMutResponse> {
        const payload: RpcCallMutRequest = {
            method: 'call_mut',
            params
        };

        return await this.request<RpcCallMutRequest, RpcCallMutResponse>(payload, config);
    }

    async request<
        Request extends RpcRequest,
        Response extends RpcResponse,
    >(rpcRequest: Request, config: RequestConfig,): Promise<Response> {
        const data: JsonRpcRequest = {
            jsonrpc: '2.0',
            id: 1,
            method: rpcRequest.method,
            params: rpcRequest.params,
        };

        try {
            const response = await this.axiosInstance.post<JsonRpcResponse<Response>>(this.path, data, config);
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
