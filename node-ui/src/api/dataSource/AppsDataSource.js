export class AppsDataSource {
  client;

  constructor(client) {
    this.client = client;
  }

  async getInstalledAplications() {
    try {
      const response = await this.client.get("/admin-api/applications");
      if (response?.apps) {
        return response.apps;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error fetching installed applications:", error);
      return [];
    }
  }
}
