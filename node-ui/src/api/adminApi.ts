import { Application } from "./dataSource/AppsDataSource";

export interface AdminApi {
  getInstalledApplications(): Promise<Application[]>;
}
