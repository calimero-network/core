export function useAdminClient() {
  const installApplication = async (selectedPackage, selectedVersion) => {
    console.log(selectedPackage, selectedVersion);
  };

  return { installApplication };
}