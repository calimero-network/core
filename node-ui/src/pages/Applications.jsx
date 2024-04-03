import { useEffect, useState } from "react";
import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { ApplicationsContent } from "../components/applications/ApplicationsContent";
import { ApplicationsTable } from "../components/applications/ApplicationsTable";
import { InstallApplication } from "../components/applications/InstallApplication";
import { useRPC } from "../hooks/useNear";
import { useAdminClient } from "../hooks/useAdminClient";
import { useUploadFile } from "../hooks/useUploadFile";

export default function Applications() {
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
  const { cidString, commitWasm, fetchWasm } = useUploadFile();

  const [myhref, setHref] = useState("");
  const handleFileChange = (event) => {
    const file = event.target.files[0];
    if (file && file.name.endsWith(".wasm")) {
      const reader = new FileReader();
      reader.onload = (e) => {
        const arrayBuffer = e.target.result;
        const bytes = new Uint8Array(arrayBuffer);
        commitWasm(bytes);
      };
      reader.readAsArrayBuffer(file);
    } else {
      console.log("Please select a .wasm file.");
    }
  };

  const handleDownload = async () => {
    let fileObject = await fetchWasm();
    console.log("🚀 ~ handleDownload ~ fileObject:", fileObject);
    setHref(fileObject);
  };
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
          <input type="file" accept=".wasm" onChange={handleFileChange} />
          <h4>{cidString}</h4>
          <button onClick={handleDownload}>download file</button>
          <a href={myhref}>download</a>
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
