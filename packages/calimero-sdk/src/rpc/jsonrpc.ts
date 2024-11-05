import {
  RpcRequestId,
  RpcClient,
  RpcQueryResponse,
  RpcQueryParams,
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

interface ErrorData {
  data: {
    data: {
      type: string;
      [key: string]: any;
    };
    type: string;
  };
  type: string;
}

interface JsonRpcResponse<Result> {
  jsonrpc: JsonRpcVersion;
  id: RpcRequestId | null;
  result?: Result;
  error?: ErrorData;
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

  // /**
  //  * @deprecated The method should not be used, use execute instead.
  //  */
  // public async query<Args, Output>(
  //   params: RpcQueryParams<Args>,
  //   config?: RequestConfig,
  // ): Promise<RpcResult<RpcQueryResponse<Output>>> {
  //   return await this.request<RpcQueryParams<Args>, RpcQueryResponse<Output>>(
  //     'execute',
  //     params,
  //     config,
  //   );
  // }

  // /**
  //  * @deprecated The method should not be used, use execute instead.
  //  */
  // public async mutate<Args, Output>(
  //   params: RpcMutateParams<Args>,
  //   config?: RequestConfig,
  // ): Promise<RpcResult<RpcMutateResponse<Output>>> {
  //   return await this.request<RpcMutateParams<Args>, RpcMutateResponse<Output>>(
  //     'execute',
  //     params,
  //     config,
  //   );
  // }

  public async execute<Args, Output>(
    params: RpcQueryParams<Args>,
    config?: RequestConfig,
  ): Promise<RpcResult<RpcQueryResponse<Output>>> {
    return await this.request<RpcQueryParams<Args>, RpcQueryResponse<Output>>(
      'execute',
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
            error: {
              code: 400,
              id: response?.data?.id,
              jsonrpc: response?.data?.jsonrpc,
              error: {
                name: 'MissmatchedRequestIdError',
                cause: {
                  name: 'MissmatchedRequestIdError',
                  info: {
                    message: `Missmatched RequestId expected ${requestId}, got ${response?.data?.id}`,
                  },
                },
              },
            },
          };
        }

        if (response?.data?.error) {
          return {
            error: {
              code: 400,
              id: response?.data?.id,
              jsonrpc: response?.data?.jsonrpc,
              error: {
                name: response?.data?.error?.type,
                cause: {
                  name:
                    response?.data?.error?.data?.type ??
                    response?.data?.error?.type,
                  info: {
                    message: response?.data?.error?.data?.data?.type,
                  },
                },
              },
            },
          };
        }
        return {
          result: response?.data?.result,
        };
      } else {
        return {
          error: {
            id: response?.data?.id,
            jsonrpc: response?.data?.jsonrpc,
            code: response?.status ?? null,
            error: {
              name: 'InvalidRequestError',
              cause: {
                name: 'InvalidRequestError',
                info: {
                  message: response?.data?.error?.data?.data?.type,
                },
              },
            },
          },
        };
      }
    } catch (error: any) {
      return {
        error: {
          id: requestId,
          jsonrpc: '2.0',
          code: error.response.code,
          error: {
            name: 'UnknownServerError',
            cause: {
              name: 'UnknownServerError',
              info: {
                message: error?.response?.data,
              },
            },
          },
        },
      };
    }
  }

  getRandomRequestId(): number {
    return Math.floor(Math.random() * Math.pow(2, 32));
  }
}
