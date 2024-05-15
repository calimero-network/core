import { Application } from "./dataSource/AppsDataSource";

export interface AdminApi {
  getInstalledAplications(): Promise<Application[]>;
}
