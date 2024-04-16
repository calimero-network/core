export class AppsDataSource {
  client;

  constructor(client) {
    this.client = client;
  }

  async getInstalledAplications() {
    return Object.keys((await this.client.get("/admin-api/applications")).apps);
  }
}
