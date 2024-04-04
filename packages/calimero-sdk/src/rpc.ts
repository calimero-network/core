export type JsonRpcVersion = '2.0'
export type JsonRpcRequestId = string | number;
export type ApplicationId = string;

export interface RpcClient {
    request(payload: RpcRequestPayload): Promise<ResponseBody>;
}

// **************************** response *******************************
export interface JsonRpcRequest {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    payload: RpcRequestPayload;
}

export type RpcRequestPayload = RpcCallPayload | RpcCallMutPayload;

export interface RpcCallPayload {
    method: 'call';
    params: RpcCallRequest;
}

export interface RpcCallMutPayload {
    method: 'call_mut';
    params: RpcCallMutRequest;
}
// *************************************************************************

// **************************** response *******************************
export interface JsonRpcResponse {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    body: ResponseBody;
}

export type ResponseBody = RpcResponseBodyResult | RpcResponseBodyError;

export interface RpcResponseBodyResult {
    result: RpcCallResponse | RpcCallMutResponse;
}

export type RpcResponseBodyError = RpcServerResponseError | RpcHandlerError;

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
    applicationId: ApplicationId;
    method: string;
    argsJson: any; // Replace with actual type
}

export interface RpcCallResponse {
    output: string | null;
}

export type RpcCallError = RpcSerdeError | RpcExecutionError;

// **************************** call_mut method ****************************
export interface RpcCallMutRequest {
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
