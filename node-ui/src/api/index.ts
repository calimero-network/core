import axios from "axios";

import { ContextDataSource } from "./dataSource/ContextDataSource";
import { AxiosHttpClient, HttpClient } from "./httpClient";
import { AdminApi } from "./adminApi";
import { ContextApi } from "./contextApi";
import { AppsDataSource } from "./dataSource/AppsDataSource";
import { DidDataSource } from "./dataSource/DidDataSource";
import { DidApi } from "./didApi";

interface IApiClient {
  admin(): AdminApi;
  context(): ContextApi;
  did(): DidApi;
}

class ApiClient implements IApiClient{
  private adminApi: AdminApi;
  private contextApi: ContextApi;
  private DidApi: DidApi;

  constructor(httpClient: HttpClient) {
    this.adminApi = new AppsDataSource(httpClient);
    this.contextApi = new ContextDataSource(httpClient);
    this.DidApi = new DidDataSource(httpClient);
  }

  admin() {
    return this.adminApi;
  }

  context() {
    return this.contextApi;
  }

  did() {
    return this.DidApi;
  }
}

const apiClient = new ApiClient(new AxiosHttpClient(axios));

export default apiClient;
