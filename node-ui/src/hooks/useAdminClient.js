import axios from "axios";

export function useAdminClient() {
  const installApplication = async (selectedPackage, selectedVersion) => {
    const response = await axios.post("/admin-api/install-application", {
      application: selectedPackage,
      version: selectedVersion,
    });
    const data = response.data;
    console.log("Response received:", data);
  };

  return { installApplication };
}
