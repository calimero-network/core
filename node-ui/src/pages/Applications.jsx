import { useEffect, useState } from "react";
import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { ApplicationsContent } from "../components/applications/ApplicationsContent";
import { ApplicationsTable } from "../components/applications/ApplicationsTable";
import { InstallApplication } from "../components/applications/InstallApplication";
import { useRPC } from "../hooks/useNear";
import { useAdminClient } from "../hooks/useAdminClient";
import { useNavigate } from "react-router-dom";

export default function Applications() {
  const navigate = useNavigate();
  const { getPackages, getReleases } = useRPC();
  const { installApplication } = useAdminClient();
  const [swithInstall, setSwitchInstall] = useState(false);
  const [selectedPackage, setSelectedPackage] = useState();
  const [selectedRelease, setSelectedRelease] = useState();
  const [packages, setPackages] = useState([]);
  const [releases, setReleases] = useState([]);

  useEffect(() => {
    if (!packages.length) {
      (async () => {
        setPackages(await getPackages());
      })();
    }
  }, [packages]);

  return (
    <FlexLayout>
      <Navigation />
      <ApplicationsContent redirectAppUpload={() => navigate("/upload-app")}>
        {swithInstall ? (
          <InstallApplication
            getReleases={getReleases}
            installApplication={installApplication}
            packages={packages}
            releases={releases}
            selectedPackage={selectedPackage}
            setReleases={setReleases}
            selectedRelease={selectedRelease}
            setSelectedRelease={setSelectedRelease}
            setSelectedPackage={setSelectedPackage}
            setSwitchInstall={setSwitchInstall}
          />
        ) : (
          <ApplicationsTable
            applications={[]}
            install={() => setSwitchInstall(true)}
            uninstall={() => console.log("uninstall ?!?")}
          />
        )}
      </ApplicationsContent>
    </FlexLayout>
  );
}
