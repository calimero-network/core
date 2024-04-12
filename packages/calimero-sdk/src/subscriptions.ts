import { ApplicationId } from './application';

export interface SubscriptionsClient {
    connect(connectionId: string): void;
    disconnect(connectionId: string): void;
    subscribe(applicationIds: string[], connectionId?: string): void;
    unsubscribe(applicationIds: string[], connectionId?: string): void;
    addCallback(callback: (data: NodeEvent) => void, connectionId?: string): void;
    removeCallback(callback: (data: NodeEvent) => void, connectionId?: string): void;
}

export type NodeEvent = ApplicationEvent;

export interface ApplicationEvent {
    application_id: ApplicationId;
    type: 'TransactionExecuted' | 'PeerJoined'
    data: TransactionExecuted | PeerJoined;
}

export interface TransactionExecuted {
    hash: string;
}

export interface PeerJoined {
    peer_id: string;
}
