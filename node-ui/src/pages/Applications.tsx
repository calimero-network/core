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

export const Tabs = {
  INSTALL_APPLICATION: 0,
  APPLICATION_LIST: 1,
};

interface Package {
  id: string;
  name: string;
  description: string;
  repository: string;
  owner: string;
}

interface Relese {
  version: string;
  notes: string;
  path: string;
  hash: string;
}

export default function Applications() {
  const t = translations.applicationsPage.installApplication;
  const navigate = useNavigate();
  const { getPackages, getReleases, getPackage } = useRPC();
  const { installApplication } = useAdminClient();
  const [selectedTab, setSelectedTab] = useState(Tabs.APPLICATION_LIST);
  const [selectedPackage, setSelectedPackage] = useState<Package | null>(null);
  const [selectedRelease, setSelectedRelease] = useState<Relese | null>(null);
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

      if (installedApplications.length !== 0) {
        const tempApplications = await Promise.all(
          installedApplications.map(async (app) => {
            const packageData = await getPackage(app.id);
            return { ...packageData, id: app.id, version: app.version };
          })
        );
        setApplications(tempApplications);
      }
    };

    setApps();
  }, [selectedTab]);

  const installApplicationHandler = async () => {
    if (!selectedPackage || !selectedRelease) {
      return;
    }
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
      setSelectedTab(Tabs.APPLICATION_LIST);
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
        {selectedTab === Tabs.INSTALL_APPLICATION ? (
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
            setSelectedTab={setSelectedTab}
            showStatusModal={showStatusModal}
            closeModal={closeStatusModal}
            installationStatus={installationStatus}
          />
        ) : (
          <ApplicationsTable
            applications={applications}
            changeTab={() => setSelectedTab(Tabs.INSTALL_APPLICATION)}
            uninstall={() => console.log("uninstall ?!?")}
          />
        )}
      </ApplicationsContent>
    </FlexLayout>
  );
}
