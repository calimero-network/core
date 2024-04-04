import { ApplicationId, RpcCallRequest, RpcClient, RpcCallPayload } from './rpc';

export class CalimeroClient {
    readonly rpcClient?: RpcClient;

    public constructor(rpcClient?: RpcClient) {
        this.rpcClient = rpcClient;
    }

    public async callMethod(applicationId: ApplicationId, method: string, argsJson: any) {
        if (!this.rpcClient) {
            throw new Error('RPC client is not set');
        }

        const request: RpcCallRequest = {
            applicationId: applicationId,
            method: method,
            argsJson: argsJson
        };
        const payload: RpcCallPayload = {
            method: 'call',
            params: request
        };

        const responseBody = await this.rpcClient.request(payload);
        return responseBody;
    }
}
