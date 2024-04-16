import { ApplicationId } from './application';

export type RpcRequestId = string | number;

export interface RpcClient {
    query<Args, Out>(params: RpcQueryParams<Args>, config?: RequestConfig): Promise<RpcResult<RpcQueryResponse<Out>>>;
    mutate<Args, Out>(params: RpcMutateParams<Args>, config?: RequestConfig): Promise<RpcResult<RpcMutateResponse<Out>>>;
}

export interface RequestConfig {
    timeout?: number
}

export type RpcResult<Result> = {
    result: Result;
    error?: null;
} | {
    result?: null;
    error: ServerError; // TODO define error types
};

export interface RpcQueryParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcQueryResponse<Output> {
    output?: Output;
}

export interface RpcMutateParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcMutateResponse<Output> {
    output?: Output;
}

export type ServerError = UnknownServerError | InvalidRequestError | MissmatchedRequestIdError | RpcExecutionError;

export interface UnknownServerError {
    type: 'UnknownServerError';
    inner: any;
}

export interface InvalidRequestError {
    type: 'InvalidRequestError';
    data: any;
    code: number;
}

export interface MissmatchedRequestIdError {
    type: 'MissmatchedRequestIdError';
    expected: RpcRequestId;
    got: RpcRequestId;
}

export interface RpcExecutionError {
    type: 'RpcExecutionError';
    inner: any;
}