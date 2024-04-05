export type JsonRpcVersion = '2.0'
export type JsonRpcRequestId = string | number;
export type ApplicationId = string;

export interface RpcClient {
    request<Request extends RpcRequest, Response extends RpcResponse>(payload: Request): Promise<Response>;
}

// **************************** request *******************************
export type RpcRequest = RpcCallRequest | RpcCallMutRequest;
export type RpcRequestParams = RpcCallRequestParams | RpcCallMutRequestParams;
// *************************************************************************

// **************************** response *******************************
export type RpcResponse = RpcCallResponse | RpcCallMutResponse | RpcCallsResponse;
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
// *************************************************************************

// **************************** call method *******************************
export interface RpcCallRequest {
    method: 'call';
    params: RpcCallRequestParams;
}

export interface RpcCallRequestParams {
    applicationId: ApplicationId;
    method: string;
    argsJson: any; // Replace with actual type
}

export interface RpcCallResponse {
    output: string | null;
}

export interface RpcCallsResponse {
    outpust: string | null;
}
export type RpcCallError = RpcSerdeError | RpcExecutionError;

// **************************** call_mut method ****************************
export interface RpcCallMutRequest {
    method: 'call_mut';
    params: RpcCallMutRequestParams;
}

export interface RpcCallMutRequestParams {
    applicationId: ApplicationId;
    method: string;
    argsJson: any; // Replace with actual type
}

export interface RpcCallMutResponse {
    output: string | null;
}

export type RpcCallMutError = RpcSerdeError | RpcExecutionError;
// *************************************************************************

// **************************** common method errors ****************************
export interface RpcSerdeError {
    type: 'SerdeError';
    message: string;
}

export interface RpcExecutionError {
    type: 'ExecutionError';
    message: string;
}
// *************************************************************************
