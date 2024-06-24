import axios from 'axios';

import { NodeApiDataSource } from './dataSource/NodeApiDataSource';
import { AxiosHttpClient, HttpClient } from './httpClient';
import { NodeApi } from './nodeApi';

class ApiClient {
  private nodeApi: NodeApi;

  constructor(httpClient: HttpClient) {
    this.nodeApi = new NodeApiDataSource(httpClient);
  }

  node() {
    return this.nodeApi;
  }
}

const apiClient = new ApiClient(new AxiosHttpClient(axios));

export default apiClient;
