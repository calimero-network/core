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
import apiClient from "../api/index";
import translations from "../constants/en.global.json";

export default function Applications() {
  const t = translations.applicationsPage.installApplication;
  const navigate = useNavigate();
  const { getPackages, getReleases, getPackage } = useRPC();
  const { installApplication } = useAdminClient();
  const [showInstallApplications, setShowInstallApplications] = useState(false);
  const [selectedPackage, setSelectedPackage] = useState();
  const [selectedRelease, setSelectedRelease] = useState();
  const [packages, setPackages] = useState([]);
  const [releases, setReleases] = useState([]);
  const [applications, setApplications] = useState([]);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [installationStatus, setInstallationStatus] = useState({
    title: "",
    message: "",
    error: false,
  });

  useEffect(() => {
    if (!packages.length) {
      (async () => {
        setPackages(await getPackages());
      })();
    }
  }, []);

  useEffect(() => {
    const setApps = async () => {
      const installedApplications = await apiClient
        .admin()
        .getInstalledAplications();

      if (Object.keys(installedApplications).length !== 0) {
        const tempApplications = await Promise.all(
          Object.keys(installedApplications).map(async (appId) => {
            const version = installedApplications[appId];
            const packageData = await getPackage(appId);
            return { ...packageData, id: appId, version: version };
          })
        );
        setApplications(tempApplications);
      }
    };

    setApps();
  }, [setShowInstallApplications]);

  const installApplicationHandler = async () => {
    const response = await installApplication(
      selectedPackage.id,
      selectedRelease.version
    );
    if (response.error) {
      setInstallationStatus({
        title: t.installErrorTitle,
        message: response.error.message,
        error: true,
      });
    } else {
      setInstallationStatus({
        title: response.data,
        message: `Installed application ${selectedPackage.name}, version ${selectedRelease.version}.`,
        error: false,
      });
    }
    setShowStatusModal(true);
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!installationStatus.error) {
      setSelectedPackage(null);
      setSelectedPackage(null);
      setShowInstallApplications(false);
    }
    setInstallationStatus({
      title: "",
      message: "",
      error: false,
    });
  };

  return (
    <FlexLayout>
      <Navigation />
      <ApplicationsContent redirectAppUpload={() => navigate("/upload-app")}>
        {showInstallApplications ? (
          <InstallApplication
            getReleases={getReleases}
            installApplication={installApplicationHandler}
            packages={packages}
            releases={releases}
            selectedPackage={selectedPackage}
            setReleases={setReleases}
            selectedRelease={selectedRelease}
            setSelectedRelease={setSelectedRelease}
            setSelectedPackage={setSelectedPackage}
            setShowInstallApplications={setShowInstallApplications}
            showStatusModal={showStatusModal}
            closeModal={closeStatusModal}
            installationStatus={installationStatus}
          />
        ) : (
          <ApplicationsTable
            applications={applications}
            install={() => setShowInstallApplications(true)}
            uninstall={() => console.log("uninstall ?!?")}
          />
        )}
      </ApplicationsContent>
    </FlexLayout>
  );
}
