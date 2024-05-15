import axios from "axios";

import { ContextDataSource } from "./dataSource/ContextDataSource";
import { AxiosHttpClient, HttpClient } from "./httpClient";
import { AdminApi } from "./adminApi";
import { ContextApi } from "./contextApi";
import { AppsDataSource } from "./dataSource/AppsDataSource";

class ApiClient {
  private adminApi: AdminApi;
  private contextApi: ContextApi;

  constructor(httpClient: HttpClient) {
    this.adminApi = new AppsDataSource(httpClient);
    this.contextApi = new ContextDataSource(httpClient);
  }

  admin() {
    return this.adminApi;
  }

  context() {
    return this.contextApi;
  }
}

const apiClient = new ApiClient(new AxiosHttpClient(axios));

export default apiClient;
