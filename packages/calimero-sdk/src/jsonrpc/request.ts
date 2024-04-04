
// export type JsonRpcRequestData = IJsonRpcData | IBatchRequest[];

// export interface IJsonRpcData {
//     internalID: string | number;
//     request: IJsonRpcRequest;
// }

// export interface IBatchRequest {
//     resolve: (data: any) => void;
//     reject: (data: any) => void;
//     request: IJsonRpcData; // IJsonRpcNotification | IJsonRpcRequest;
// }

// export interface IJsonRpcRequest {
//     jsonrpc: "2.0";
//     id: string | number;
//     method: string;
//     params?: object;
// }

// export interface IJsonRpcError {
//     code: number;
//     message: string;
//     data: any;
// }

// export interface IJsonRpcResponse {
//     jsonrpc: "2.0";
//     id?: string | number; // can also be null
//     result?: any;
//     error?: IJsonRpcError;
// }
