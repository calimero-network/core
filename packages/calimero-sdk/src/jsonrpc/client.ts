import { IJsonRpcRequest } from "./request";
import { ITransport } from "./transports/transport";

export class JsonRpcClient {
    readonly transport: ITransport;

    public constructor(transport: ITransport) {
        this.transport = transport;
    }

    public async request(method: string, params?: object) {
        const data: IJsonRpcRequest = {
            jsonrpc: '2.0',
            id: 1,
            method: method,
            params: params,
        };

        return this.transport.sendData(data);
    }
}