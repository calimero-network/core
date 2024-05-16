import axios from "axios";

export function useAdminClient() {
  const installApplication = async (selectedPackage: string, selectedVersion: string) => {
    try {
      const response = await axios.post("/admin-api/install-application", {
        application: selectedPackage,
        version: selectedVersion,
      });
      return { data: response?.data };
    } catch (error) {
      // @ts-ignore: Property 'response' does not exist on type 'unknown'
      // TODO: add error type
      if (error.response) {
        return {
          error: {
            // @ts-ignore
            code: error.response.status,
            // @ts-ignore:
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
