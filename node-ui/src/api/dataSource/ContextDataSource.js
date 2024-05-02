export class ContextDataSource {
  client;

  constructor(client) {
    this.client = client;
  }

  async getContexts() {
    try {
      const response = await this.client.get("/admin-api/context");
      if (response?.contexts) {
        return response.contexts;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error fetching contexts:", error);
      return [];
    }
  }

  async startContexts(applicationId, initFunction, initArguments) {
    try {
      const response = await this.client.post("/admin-api/context", {
        appId: applicationId,
        ...(initFunction && { initFunction }),
        ...(initArguments && { initArgs: JSON.stringify(initArguments) }),
      });
      if (response?.data) {
        return response.data;
      } else {
        return [];
      }
    } catch (error) {
      console.error("Error starting contexts:", error);
      return [];
    }
  }
}
