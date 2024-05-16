import { Application } from "./dataSource/NodeDataSource";
import { Context, NodeContexts } from "./dataSource/NodeDataSource";

export interface NodeApi {
  getInstalledApplications(): Promise<Application[]>;
  getContexts(): Promise<NodeContexts<Context>>;
  getContext(contextId: string): Promise<Context | null>;
  deleteContext(contextId: string): Promise<Boolean>;
  startContexts(
    applicationId: string,
    initFunction: string,
    initArguments: string
  ): Promise<boolean>;
}
