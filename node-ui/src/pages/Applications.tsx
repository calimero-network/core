import { useEffect, useState } from "react";
import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { InstallApplication } from "../components/applications/InstallApplication";
import { useRPC } from "../hooks/useNear";
import { useAdminClient } from "../hooks/useAdminClient";
import { useNavigate } from "react-router-dom";
import apiClient from "../api/index";
import translations from "../constants/en.global.json";
import { ModalContent } from "../components/common/StatusModal";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ApplicationsTable from "../components/applications/ApplicationsTable";
import { TableOptions } from "../components/common/OptionsHeader";
import { ApplicationOptions } from "../constants/ContextConstants";

export enum Tabs {
  INSTALL_APPLICATION,
  APPLICATION_LIST,
}

export interface Package {
  id: string;
  name: string;
  description: string;
  repository: string;
  owner: string;
}

export interface Release {
  version: string;
  notes: string;
  path: string;
  hash: string;
}

export interface NodeApp {
  id: string;
  version: string;
}

export interface Application extends Package {
  version: string;
}

const initialOptions = [
  {
    name: "Available",
    id: ApplicationOptions.AVAILABLE,
    count: 0,
  },
  {
    name: "Owned",
    id: ApplicationOptions.OWNED,
    count: 0,
  },
];

export interface Applications {
  available: Application[];
  owned: Application[];
}

export default function Applications() {
  const t = translations.applicationsPage.installApplication;
  const navigate = useNavigate();
  const { getPackages, getReleases, getPackage } = useRPC();
  const { installApplication } = useAdminClient();
  const [selectedTab, setSelectedTab] = useState(Tabs.APPLICATION_LIST);
  const [currentOption, setCurrentOption] = useState<string>(
    ApplicationOptions.AVAILABLE
  );
  const [tableOptions, setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const [selectedPackage, setSelectedPackage] = useState<Package | null>(null);
  const [selectedRelease, setSelectedRelease] = useState<Release | null>(null);
  const [packages, setPackages] = useState<Package[]>([]);
  const [releases, setReleases] = useState<Release[]>([]);
  const [applications, setApplications] = useState<Applications>({
    available: [],
    owned: [],
  });
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [installationStatus, setInstallationStatus] = useState<ModalContent>({
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
        .node()
        .getInstalledApplications();

      if (installedApplications.length !== 0) {
        const tempApplications = await Promise.all(
          installedApplications.map(async (app: NodeApp) => {
            const packageData = await getPackage(app.id);
            return { ...packageData, id: app.id, version: app.version };
          })
        );
        setApplications((prevState: Applications) => ({
          ...prevState,
          available: tempApplications,
        }));
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
      <PageContentWrapper>
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
            applicationsList={applications}
            currentOption={currentOption}
            setCurrentOption={setCurrentOption}
            tableOptions={tableOptions}
            naviagateToAppDetails={(id: string) =>
              navigate(`/applications/${id}`)
            }
            naviagateToPublishApp={() => navigate("/upload-app")}
          />
        )}
      </PageContentWrapper>
    </FlexLayout>
  );
}
