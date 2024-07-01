import { ContextId } from '../types/context';

export interface SubscriptionsClient {
  connect(connectionId: string): void;
  disconnect(connectionId: string): void;
  subscribe(applicationIds: string[], connectionId?: string): void;
  unsubscribe(applicationIds: string[], connectionId?: string): void;
  addCallback(callback: (data: NodeEvent) => void, connectionId?: string): void;
  removeCallback(
    callback: (data: NodeEvent) => void,
    connectionId?: string,
  ): void;
}

export type NodeEvent = ApplicationEvent;

export interface ApplicationEvent {
  context_id: ContextId;
  type: 'TransactionExecuted' | 'PeerJoined';
  data: TransactionExecuted | PeerJoined;
}

export interface TransactionExecuted {
  hash: string;
}

export interface PeerJoined {
  peerId: string;
}
