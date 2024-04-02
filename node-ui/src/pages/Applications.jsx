import { useEffect, useState } from "react";
import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { ApplicationsContent } from "../components/applications/ApplicationsContent";
import { ApplicationsTable } from "../components/applications/ApplicationsTable";
import { InstallApplication } from "../components/applications/InstallApplication";
import { useRPC } from "../hooks/useNear";

export default function Applications() {
  const { getPackages, getReleases, installApplication } = useRPC();
  const [swithInstall, setSwitchInstall] = useState(true);
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
      {swithInstall ? (
        <ApplicationsContent>
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
        </ApplicationsContent>
      ) : (
        <ApplicationsContent>
          <ApplicationsTable
            applications={[]}
            install={() => setSwitchInstall(true)}
            uninstall={() => console.log("uninstall ?!?")}
          />
        </ApplicationsContent>
      )}
    </FlexLayout>
  );
}
