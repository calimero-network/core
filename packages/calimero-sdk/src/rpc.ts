import { ApplicationId } from './application';

export interface RpcClient {
    query<Args, Out>(params: RpcQueryParams<Args>, config?: RequestConfig): Promise<RpcQueryResponse<Out>>;
    mutate<Args, Out>(params: RpcMutateParams<Args>, config?: RequestConfig): Promise<RpcMutateResponse<Out>>;
}

export interface RequestConfig {
    timeout?: number
}

export type RpcResponseError = RpcServerResponseError | RpcHandlerError;

export interface RpcHandlerError {
    handlerError: any; // Replace with actual type
}

export type RpcServerResponseError = RpcParseError | RpcInternalError;

export interface RpcParseError {
    type: 'ParseError';
    data: string;
}

export interface RpcInternalError {
    type: 'InternalError';
    data: {
        err: any; // Replace with actual type
    };
}

export interface RpcQueryParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcQueryResponse<Out> {
    output?: Out;
}

export type RpcQueryError = RpcSerdeError | RpcExecutionError;

export interface RpcMutateParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcMutateResponse<Out> {
    output?: Out;
}

export type RpcMutateError = RpcSerdeError | RpcExecutionError;

export interface RpcSerdeError {
    type: 'SerdeError';
    message: string;
}

export interface RpcExecutionError {
    type: 'ExecutionError';
    message: string;
}
