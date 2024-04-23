import axios from "axios";

export function useAdminClient() {
  const installApplication = async (selectedPackage, selectedVersion) => {
    try {
      const response = await axios.post("/admin-api/install-application", {
        application: selectedPackage,
        version: selectedVersion,
      });
      return { data: response?.data };
    } catch (error) {
      if (error.response) {
        return {
          error: {
            code: error.response.status,
            message: error.response.data?.error,
          },
        };
      } else {
        return {
          error: {
            code: 500,
            message: "Try again later.",
          }
        }
      }
    }
  };

  return { installApplication };
}
