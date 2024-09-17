import { useEffect, useState } from 'react';
import React from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import { useRPC } from '../hooks/useNear';
import { useNavigate } from 'react-router-dom';
import apiClient from '../api/index';
import PageContentWrapper from '../components/common/PageContentWrapper';
import ApplicationsTable from '../components/applications/ApplicationsTable';
import { TableOptions } from '../components/common/OptionsHeader';
import { ApplicationOptions } from '../constants/ContextConstants';
import {
  Application,
  GetInstalledApplicationsResponse,
  InstalledApplication,
} from '../api/dataSource/NodeDataSource';
import { ResponseData } from '../api/response';
import { useServerDown } from '../context/ServerDownContext';
import { AppMetadata, parseAppMetadata } from '../utils/metadata';

export enum Tabs {
  AVAILABLE,
  OWNED,
  INSTALLED,
}

export interface Package {
  id: string; // this is contract app id
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

const initialOptions = [
  {
    name: 'Available',
    id: ApplicationOptions.AVAILABLE,
    count: 0,
  },
  {
    name: 'Owned',
    id: ApplicationOptions.OWNED,
    count: 0,
  },
  {
    name: 'Installed',
    id: ApplicationOptions.INSTALLED,
    count: 0,
  },
];

export interface Applications {
  available: Application[];
  owned: Application[];
  installed: Application[];
}

export default function ApplicationsPage() {
  const navigate = useNavigate();
  const { showServerDownPopup } = useServerDown();
  const { getPackages, getLatestRelease, getPackage } = useRPC();
  const [errorMessage, setErrorMessage] = useState('');
  const [currentOption, setCurrentOption] = useState<string>(
    ApplicationOptions.AVAILABLE,
  );
  const [tableOptions] = useState<TableOptions[]>(initialOptions);
  const [applications, setApplications] = useState<Applications>({
    available: [],
    owned: [],
    installed: [],
  });

  useEffect(() => {
    const setApplicationsList = async () => {
      const packages = await getPackages();
      if (packages.length !== 0) {
        var tempApplications: Application[] = await Promise.all(
          packages.map(async (appPackage: Package) => {
            const releaseData = await getLatestRelease(appPackage.id);

            const application: Application = {
              id: appPackage.id,
              name: appPackage.name,
              description: appPackage.description,
              repository: appPackage.repository,
              owner: appPackage.owner,
              version: releaseData?.version ?? '',
              blob: '',
              source: '',
              contract_app_id: appPackage.id,
            };
            return application;
          }),
        );

        //remove all apps without release
        tempApplications = tempApplications.filter((app) => app.version !== '');

        setApplications((prevState: Applications) => ({
          ...prevState,
          available: tempApplications,
        }));
      }
    };
    setApplicationsList();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const setApps = async () => {
      setErrorMessage('');
      const fetchApplicationResponse: ResponseData<GetInstalledApplicationsResponse> =
        await apiClient(showServerDownPopup).node().getInstalledApplications();

      if (fetchApplicationResponse.error) {
        setErrorMessage(fetchApplicationResponse.error.message);
        return;
      }
      let installedApplications = fetchApplicationResponse.data?.apps;
      if (installedApplications.length !== 0) {
        var tempApplications: (Application | null)[] = await Promise.all(
          installedApplications.map(
            async (app: InstalledApplication): Promise<Application | null> => {
              var appMetadata: AppMetadata | null = parseAppMetadata(
                app.metadata,
              );

              let application: Application | null = null;
              if (!appMetadata) {
                application = {
                  id: app.id,
                  version: app.version,
                  source: app.source,
                  blob: app.blob,
                  contract_app_id: null,
                  name: 'local app',
                  description: null,
                  repository: null,
                  owner: null,
                };
              } else {
                const packageData: Package | null = await getPackage(
                  appMetadata.contractAppId,
                );

                if (packageData) {
                  application = {
                    ...app,
                    contract_app_id: appMetadata.contractAppId,
                    name: packageData?.name ?? '',
                    description: packageData?.description,
                    repository: packageData?.repository,
                    owner: packageData?.owner,
                  };
                }
              }

              return application;
            },
          ),
        );
        var installed: Application[] = tempApplications.filter(
          (app): app is Application => app !== null,
        );

        setApplications((prevState: Applications) => ({
          ...prevState,
          installed,
        }));
      }
    };

    setApps();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ApplicationsTable
          applicationsList={applications}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
          navigateToAppDetails={(app: Application | undefined) => {
            if (app) {
              navigate(`/applications/${app.id}`);
            }
          }}
          navigateToPublishApp={() => navigate('/publish-application')}
          navigateToInstallApp={() => navigate('/applications/install')}
          errorMessage={errorMessage}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
