import { IJsonRpcRequest } from "../request";
import { ITransport } from "./transport";
import axios, { AxiosInstance } from "axios";

export class HttpTransport implements ITransport {
    readonly path: string;
    readonly axiosInstance: AxiosInstance;

    public constructor(baseUrl: string, path: string, defaultTimeout: number = 1000) {
        this.path = path;
        this.axiosInstance = axios.create({
            baseURL: baseUrl,
            timeout: defaultTimeout,
        });
    }

    public async sendData(data: IJsonRpcRequest, timeout?: number | null | undefined): Promise<any> {
        let requestConfig: any = {};
        if (typeof timeout !== 'undefined' && timeout !== null) {
            requestConfig.timeout = timeout;
        }

        try {
            return await this.axiosInstance.post(this.path, data, requestConfig);
        } catch (error: any) {
            throw new Error("Post request failed: " + error.message);
        }
    }
}