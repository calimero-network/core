import axios from "axios";

export function useAdminClient() {
  const installApplication = async (selectedPackage, selectedVersion) => {
    try {
      const response = await axios.post("/admin-api/install-application", {
        application: selectedPackage,
        version: selectedVersion,
      });
      if (response.status === 200) {
        return { data: response.data };
      } else {
        return {
          error: { code: 500, message: "Failed to install application" },
        };
      }
    } catch (error) {
      if (error.response) {
        return {
          error: {
            code: error.response.status,
            message: error.response.data?.error,
          },
        };
      }
    }
  };

  return { installApplication };
}
