import { RpcClient, RpcRequest, RpcResponse, RpcCallRequest, RpcCallResponse, RpcCallMutRequest, RpcCallMutResponse } from "../rpc";
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

    public async call(applicationId: string, method: string, argsJson: object): Promise<any[] | object> {
        const payload: RpcCallRequest = {
            method: 'call',
            params: {
                applicationId: applicationId,
                method: method,
                argsJson: argsJson
            }
        };

        const response = await this.request<RpcCallRequest, RpcCallResponse>(payload);
        return JSON.parse(response.output);
    }

    public async callMut(applicationId: string, method: string, argsJson: object): Promise<any[] | object> {
        const payload: RpcCallMutRequest = {
            method: 'call_mut',
            params: {
                applicationId: applicationId,
                method: method,
                argsJson: argsJson
            }
        };

        const response = await this.request<RpcCallMutRequest, RpcCallMutResponse>(payload);
        return JSON.parse(response.output);
    }

    public async request<
        Request extends RpcRequest,
        Response extends RpcResponse,
    >(rpcRequest: Request, timeout?: number): Promise<Response> {
        const data: JsonRpcRequest = {
            jsonrpc: '2.0',
            id: 1,
            method: rpcRequest.method,
            params: rpcRequest.params,
        };

        let requestConfig: any = {};
        if (typeof timeout !== 'undefined' && timeout !== null) {
            requestConfig.timeout = timeout;
        }

        try {
            const response = await this.axiosInstance.post<JsonRpcResponse<Response>>(this.path, data, requestConfig);
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