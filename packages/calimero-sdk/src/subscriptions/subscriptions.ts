import { ContextId } from '../types/context';

export interface SubscriptionsClient {
  connect(connectionId?: string): Promise<void>;
  disconnect(connectionId?: string): void;
  subscribe(contextIds: string[], connectionId?: string): void;
  unsubscribe(contextIds: string[], connectionId?: string): void;
  addCallback(callback: (data: NodeEvent) => void, connectionId?: string): void;
  removeCallback(
    callback: (data: NodeEvent) => void,
    connectionId?: string,
  ): void;
}

export type NodeEvent = ApplicationEvent;

export interface ApplicationEvent {
  context_id: ContextId;
  type: 'TransactionExecuted';
  data: OutcomeEvents;
}

export interface OutcomeEvent {
  kind: String;
  data: number[];
}

export interface OutcomeEvents {
  events: OutcomeEvent[];
}
