import { JsonRpcVersion, JsonRpcRequestId, RpcRequestParams, RpcResponse, RpcResponseError } from "../rpc";

export interface JsonRpcRequest {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    method: string;
    params: RpcRequestParams;
}

export interface JsonRpcResponse<T extends RpcResponse> {
    jsonrpc: JsonRpcVersion;
    id: JsonRpcRequestId | null;
    result?: T;
    error?: RpcResponseError;
}
