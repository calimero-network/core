export class ContextDataSource {
  client;

  constructor(client) {
    this.client = client;
  }

  async getContexts() {
    try {
      const response = await this.client.get("/admin-api/contexts");
      if (response?.data) {
        // invited is empty for now as we don't have this endpoint available
        // will be left as "no invites" until this becomes available
        return {
          joined: response.data,
          invited: [],
        };
      } else {
        return { joined: [], invited: [] };
      }
    } catch (error) {
      console.error("Error fetching contexts:", error);
      return { joined: [], invited: [] };
    }
  }

  async getContext(contextId) {
    try {
      const response = await this.client.get(
        `/admin-api/contexts/${contextId}`
      );
      if (response?.data) {
        return response.data.context;
      } else {
        return false;
      }
    } catch (error) {
      console.error("Error fetching context:", error);
      return false;
    }
  }

  async deleteContext(contextId) {
    try {
      const response = await this.client.delete(
        `/admin-api/contexts/${contextId}`
      );
      if (response?.data) {
        return response.data;
      } else {
        return false;
      }
    } catch (error) {
      console.error("Error deleting context:", error);
      return false;
    }
  }

  async startContexts(applicationId, initFunction, initArguments) {
    try {
      const response = await this.client.post("/admin-api/contexts", {
        applicationId: applicationId,
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
