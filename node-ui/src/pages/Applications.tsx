import { useEffect, useState } from "react";
import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { useRPC } from "../hooks/useNear";
import { useNavigate } from "react-router-dom";
import apiClient from "../api/index";
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
  const navigate = useNavigate();
  const { getPackages, getPackage } = useRPC();
  const [selectedTab, setSelectedTab] = useState(Tabs.APPLICATION_LIST);
  const [currentOption, setCurrentOption] = useState<string>(
    ApplicationOptions.AVAILABLE
  );
  const [tableOptions, _setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const [packages, setPackages] = useState<Package[]>([]);
  const [applications, setApplications] = useState<Applications>({
    available: [],
    owned: [],
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

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
          <ApplicationsTable
            applicationsList={applications}
            currentOption={currentOption}
            setCurrentOption={setCurrentOption}
            tableOptions={tableOptions}
            navigateToAppDetails={(id: string) =>
              navigate(`/applications/${id}`)
            }
            navigateToPublishApp={() => navigate("/publish-application")}
            changeSelectedTab={() => setSelectedTab(Tabs.INSTALL_APPLICATION)}
          />
      </PageContentWrapper>
    </FlexLayout>
  );
}
