import { SubscriptionManager, NodeEvent } from '../subscriptions';

const DEFAULT_CONNECTION_ID = "DEFAULT";

export type WsRequestId = string | number;

interface WsRequest<Params> {
    id: WsRequestId | null;
    method: string;
    params: Params;
}

interface WsResponse {
    id: WsRequestId | null;
    result?: any;
    error?: any;
}

interface SubscribeRequest {
    applicationIds: string[];
}

interface UnsubscribeRequest {
    applicationIds: string[];
}

export class WebSocketSubscriptionManager implements SubscriptionManager {
    private readonly url: string;
    private connections: Map<string, WebSocket>;
    private callbacks: Map<string, Array<(data: NodeEvent) => void>>;

    public constructor(baseUrl: string, path: string,) {
        this.url = `${baseUrl}${path}`;
        this.connections = new Map();
        this.callbacks = new Map();
    }

    public connect(connectionId: string = DEFAULT_CONNECTION_ID): Promise<void> {
        return new Promise((resolve, reject) => {
            const websocket = new WebSocket(this.url);
            this.connections.set(connectionId, websocket);
            this.callbacks.set(connectionId, []);

            websocket.onopen = () => {
                resolve();
            };
            websocket.onerror = (error) => {
                reject(error);
            };
            websocket.onmessage = (event) => this.handleMessage(connectionId, event);
        });
    }

    public disconnect(connectionId: string = DEFAULT_CONNECTION_ID): void {
        const websocket = this.connections.get(connectionId);
        if (websocket) {
            websocket.close();
            this.connections.delete(connectionId);
            this.callbacks.delete(connectionId);
        }
    }

    public subscribe(applicationIds: string[], connectionId: string = DEFAULT_CONNECTION_ID): void {
        const websocket = this.connections.get(connectionId);
        if (websocket && websocket.readyState === websocket.OPEN) {
            console.log("subscribing to", applicationIds);
            const request: WsRequest<SubscribeRequest> = {
                id: this.getRandomRequestId(),
                method: 'subscribe',
                params: {
                    applicationIds: applicationIds
                }
            };
            try {
                websocket.send(JSON.stringify(request));
            } catch (error) {
                console.error("Error while sending subscription request", error);
            }
        }
    }

    public unsubscribe(applicationIds: string[], connectionId: string = DEFAULT_CONNECTION_ID): void {
        const websocket = this.connections.get(connectionId);
        if (websocket && websocket.readyState === websocket.OPEN) {
            const request: WsRequest<UnsubscribeRequest> = {
                id: this.getRandomRequestId(),
                method: 'unsubscribe',
                params: {
                    applicationIds: applicationIds
                }
            };
            websocket.send(JSON.stringify(request));
        }
    }

    public addCallback(callback: (data: NodeEvent) => void, connectionId: string = DEFAULT_CONNECTION_ID): void {
        const callbacks = this.callbacks.get(connectionId);
        if (callbacks) {
            callbacks.push(callback);
        }
    }

    public removeCallback(callback: (data: NodeEvent) => void, connectionId: string = DEFAULT_CONNECTION_ID): void {
        const callbacks = this.callbacks.get(connectionId);
        if (callbacks) {
            const index = callbacks.indexOf(callback);
            if (index !== -1) {
                callbacks.splice(index, 1);
            }
        }
    }

    private handleMessage(connection_id: string, event: any): void {
        if (event.type !== "text") {
            return;
        }

        const message: WsResponse = JSON.parse(event.data.toString());
        if (message.id !== null) {
            // TODO: handle non event messages gracefully
            return;
        }
        if (message.error !== null) {
            // TODO: handle errors gracefully
            return;
        }

        const callbacks = this.callbacks.get(connection_id);
        if (callbacks) {
            for (const callback of callbacks) {
                const nodeEvent: NodeEvent = message.result;
                console.log("received event", nodeEvent);
                callback(nodeEvent);
            }
        }
    }

    private getRandomRequestId(): number {
        return Math.floor(Math.random() * Math.pow(2, 32));
    }
}
