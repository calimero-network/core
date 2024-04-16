import axios from "axios";

export function useAdminClient() {
  const installApplication = async (selectedPackage, selectedVersion) => {
    const response = await axios.post("/admin-api/install-application", {
      application: selectedPackage,
      version: selectedVersion,
    });
    return response.data;
  };

  return { installApplication };
}
