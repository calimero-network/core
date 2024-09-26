import {
  RpcRequestId,
  RpcClient,
  RpcQueryResponse,
  RpcMutateResponse,
  RpcQueryParams,
  RpcMutateParams,
  RequestConfig,
  RpcResult,
} from '../types/rpc';
import axios, { AxiosInstance } from 'axios';

type JsonRpcVersion = '2.0';

interface JsonRpcRequest<Params> {
  jsonrpc: JsonRpcVersion;
  id: RpcRequestId | null;
  method: string;
  params: Params;
}

interface RpcError {
  message: string;
  code: number;
}

interface JsonRpcResponse<Result> {
  jsonrpc: JsonRpcVersion;
  id: RpcRequestId | null;
  result?: Result;
  error?: RpcError;
}

export class JsonRpcClient implements RpcClient {
  readonly path: string;
  readonly axiosInstance: AxiosInstance;

  public constructor(
    baseUrl: string,
    path: string,
    defaultTimeout: number = 1000,
  ) {
    this.path = path;
    this.axiosInstance = axios.create({
      baseURL: baseUrl,
      timeout: defaultTimeout,
    });
  }

  public async query<Args, Output>(
    params: RpcQueryParams<Args>,
    config?: RequestConfig,
  ): Promise<RpcResult<RpcQueryResponse<Output>>> {
    return await this.request<RpcQueryParams<Args>, RpcQueryResponse<Output>>(
      'query',
      params,
      config,
    );
  }

  public async mutate<Args, Output>(
    params: RpcMutateParams<Args>,
    config?: RequestConfig,
  ): Promise<RpcResult<RpcMutateResponse<Output>>> {
    return await this.request<RpcMutateParams<Args>, RpcMutateResponse<Output>>(
      'mutate',
      params,
      config,
    );
  }

  async request<Params, Result>(
    method: string,
    params: Params,
    config?: RequestConfig,
  ): Promise<RpcResult<Result>> {
    const requestId = this.getRandomRequestId();
    const data: JsonRpcRequest<Params> = {
      jsonrpc: '2.0',
      id: requestId,
      method,
      params,
    };

    try {
      const response = await this.axiosInstance.post<JsonRpcResponse<Result>>(
        this.path,
        data,
        config,
      );
      if (response?.status === 200) {
        if (response?.data?.id !== requestId) {
          return {
            result: null,
            error: {
              type: 'MissmatchedRequestIdError',
              expected: requestId ?? null,
              got: response?.data?.id ?? null,
            },
          };
        }

        if (response?.data?.error) {
          return {
            result: null,
            error: {
              type: 'RpcExecutionError',
              inner: response?.data?.error ?? null,
              code: response?.status ?? null,
              message: response?.data?.error?.message ?? null,
            },
          };
        }
        return {
          result: response?.data?.result ?? null,
          error: null,
        };
      } else {
        return {
          result: null,
          error: {
            type: 'InvalidRequestError',
            data: response?.data ?? null,
            code: response?.status ?? null,
          },
        };
      }
    } catch (error: any) {
      return {
        result: null,
        error: {
          type: 'UnknownServerError',
          inner: error,
          code: error?.response?.status ?? null,
          message: error?.response?.data ?? null,
        },
      };
    }
  }

  getRandomRequestId(): number {
    return Math.floor(Math.random() * Math.pow(2, 32));
  }
}
