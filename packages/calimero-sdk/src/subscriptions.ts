import { ApplicationId } from './application';

export type NodeEvent = {
    type: 'application_event';
    payload: ApplicationEventPayload;
};

export interface ApplicationEventPayload {
    application_id: ApplicationId;
    type: 'transaction_executed' | 'peer_joined'
    event: TransactionExecuted | PeerJoined;
}

export interface TransactionExecuted {
    hash: string;
}

export interface PeerJoined {
    peer_id: string;
}

export interface SubscriptionManager {
    connect(connectionId: string): void;
    disconnect(connectionId: string): void;
    subscribe(applicationIds: string[], connectionId: string): void;
    unsubscribe(applicationIds: string[], connectionId: string): void;
    addCallback(callback: (data: NodeEvent) => void, connectionId: string): void;
    removeCallback(callback: (data: NodeEvent) => void, connectionId: string): void;
}
