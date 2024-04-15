import axios from "axios";

import { AppsDataSource } from "./dataSource/appsDataSource";
import { AxiosHttpClient } from "./httpClient";

class ApiClient {
  adminApi;

  constructor(httpClient) {
    this.adminApi = new AppsDataSource(httpClient);
  }

  admin() {
    return this.adminApi;
  }
}

const apiClient = new ApiClient(new AxiosHttpClient(axios));

export default apiClient;
