import { ResponseBody, RpcClient, RpcRequestPayload, JsonRpcRequest, JsonRpcResponse } from "../rpc";
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

    public async request(payload: RpcRequestPayload, timeout?: number): Promise<ResponseBody> {
        const data: JsonRpcRequest = {
            jsonrpc: '2.0',
            id: 1,
            payload: payload,
        };

        let requestConfig: any = {};
        if (typeof timeout !== 'undefined' && timeout !== null) {
            requestConfig.timeout = timeout;
        }

        try {
            const response = await this.axiosInstance.post<JsonRpcResponse>(this.path, data, requestConfig);
            return response.data.body;
        } catch (error: any) {
            throw new Error("Post request failed: " + error.message);
        }
    }
}