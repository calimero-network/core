import axios from "axios";

import { AppsDataSource } from "./dataSource/AppsDataSource";
import { ContextDataSource } from "./dataSource/ContextDataSource";
import { AxiosHttpClient } from "./httpClient";

class ApiClient {
  adminApi;

  constructor(httpClient) {
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
