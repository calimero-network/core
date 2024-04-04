import { IJsonRpcRequest } from "../request";

export interface ITransport {
    sendData(data: IJsonRpcRequest, timeout?: number | null): Promise<any>;
}