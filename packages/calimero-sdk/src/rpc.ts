export type JsonRpcVersion = '2.0'
export type JsonRpcRequestId = string | number;
export type ApplicationId = string;

export interface RpcClient {
    call<Args, Out>(params: RpcCallRequestParams<Args>, config: RequestConfig): Promise<RpcCallResponse<Out>>;
    callMut<Args, Out>(params: RpcCallMutRequestParams<Args>, config: RequestConfig): Promise<RpcCallMutResponse<Out>>;
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

export interface RpcCallRequestParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcCallResponse<T> {
    output?: T;
}

export type RpcCallError = RpcSerdeError | RpcExecutionError;

export interface RpcCallMutRequestParams<Args> {
    applicationId: ApplicationId;
    method: string;
    argsJson: Args;
}

export interface RpcCallMutResponse<Out> {
    output?: Out;
}

export type RpcCallMutError = RpcSerdeError | RpcExecutionError;

export interface RpcSerdeError {
    type: 'SerdeError';
    message: string;
}

export interface RpcExecutionError {
    type: 'ExecutionError';
    message: string;
}
