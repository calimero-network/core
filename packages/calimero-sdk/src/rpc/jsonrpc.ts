
import {
    RpcRequestId,
    RpcClient,
    RpcQueryResponse,
    RpcMutateResponse,
    RpcQueryParams,
    RpcMutateParams,
    RequestConfig,
    RpcResponseResult,
} from "../rpc";
import axios, { AxiosInstance } from "axios";

type JsonRpcVersion = '2.0'

interface JsonRpcRequest<Params> {
    jsonrpc: JsonRpcVersion;
    id: RpcRequestId | null;
    method: string;
    params: Params;
}

interface JsonRpcResponse<Result> {
    jsonrpc: JsonRpcVersion;
    id: RpcRequestId | null;
    result?: Result;
    error?: any; // TODO define error types
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

    public async query<Args, Output>(params: RpcQueryParams<Args>, config?: RequestConfig): Promise<RpcResponseResult<RpcQueryResponse<Output>>> {
        return await this.request<RpcQueryParams<Args>, RpcQueryResponse<Output>>('query', params, config);
    }

    public async mutate<Args, Output>(params: RpcMutateParams<Args>, config?: RequestConfig): Promise<RpcResponseResult<RpcMutateResponse<Output>>> {
        return await this.request<RpcMutateParams<Args>, RpcMutateResponse<Output>>('mutate', params, config);
    }

    async request<Params, Result>(method: string, params: Params, config?: RequestConfig): Promise<RpcResponseResult<Result>> {
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
                if (response.data.id !== requestId) {
                    return {
                        result: null,
                        error: {
                            type: 'MissmatchedRequestIdError',
                            expected: requestId,
                            got: response.data.id,
                        },
                    };
                }

                if (response.data.error) {
                    return {
                        result: null,
                        error: {
                            type: 'RpcExecutionError',
                            inner: response.data.error,
                        },
                    };
                }

                return {
                    result: response.data.result,
                    error: null,
                };
            } else {

                return {
                    result: null,
                    error: {
                        type: 'InvalidRequestError',
                        data: response.data,
                        code: response.status,
                    },
                };
            }
        } catch (error: any) {
            return {
                result: null,
                error: {
                    type: 'UnknownServerError',
                    inner: error,
                },
            };
        }
    }

    getRandomRequestId(): number {
        return Math.floor(Math.random() * Math.pow(2, 32));
    }
}
