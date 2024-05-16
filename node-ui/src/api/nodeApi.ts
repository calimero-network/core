import { Application } from "./dataSource/NodeDataSource";
import { Context, ContextsList } from "./dataSource/NodeDataSource";

export interface NodeApi {
  getInstalledApplications(): Promise<Application[]>;
  getContexts(): Promise<ContextsList<Context>>;
  getContext(contextId: string): Promise<Context | null>;
  deleteContext(contextId: string): Promise<Boolean>;
  startContexts(
    applicationId: string,
    initFunction: string,
    initArguments: string
  ): Promise<boolean>;
}
