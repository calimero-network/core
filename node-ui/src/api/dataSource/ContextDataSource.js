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

    async startContexts(applicationId, initArguments) {
        const argumentsStringify = JSON.stringify(initArguments);

        try {
            const response = await this.client.post("/admin-api/context", {applicationId, argumentsStringify});
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
  