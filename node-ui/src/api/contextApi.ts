import { Context, NodeContexts } from "./dataSource/ContextDataSource";

export interface ContextApi {
  getContexts(): Promise<NodeContexts<Context>>;
  getContext(contextId: string): Promise<Context>;
  deleteContext(contextId: string): Promise<Boolean>;
  startContexts(
    applicationId: string,
    initFunction: string,
    initArguments: string
  ): Promise<boolean>;
}
