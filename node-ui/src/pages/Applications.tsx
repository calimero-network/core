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
import { ModalContent } from '../components/common/StatusModal';

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
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [showActionDialog, setShowActionDialog] = useState(false);
  const [selectedAppId, setSelectedAppId] = useState('');
  const [uninstallStatus, setUninstallStatus] = useState<ModalContent>({
    title: '',
    message: '',
    error: false,
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
              const [packageData, releaseData]: [
                Package | null,
                Release | null,
              ] = await Promise.all([
                getPackage(appMetadata.contractAppId),
                getLatestRelease(appMetadata.contractAppId),
              ]);

              if (packageData) {
                application = {
                  ...app,
                  contract_app_id: appMetadata.contractAppId,
                  name: packageData?.name ?? '',
                  description: packageData?.description,
                  repository: packageData?.repository,
                  owner: packageData?.owner,
                  version: releaseData?.version ?? '',
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

  useEffect(() => {
    setApps();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const uninstallApplication = async () => {
    const contextResponse = await apiClient(showServerDownPopup)
      .node()
      .getContexts();
    if (contextResponse.error) {
      setUninstallStatus({
        title: 'Error',
        message: contextResponse.error.message,
        error: true,
      });
      setShowActionDialog(false);
      setShowStatusModal(true);
      return;
    }

    const contextList = contextResponse.data?.contexts;

    if (contextList && contextList.length !== 0) {
      const contextList = contextResponse.data?.contexts;
      const usedByContextsIds =
        contextList
          ?.filter((context) => context.applicationId === selectedAppId)
          .map((context) => context.id) ?? [];

      if (usedByContextsIds.length !== 0) {
        setUninstallStatus({
          title: 'This application cannot be uninstalled',
          message: `This application is used by the following contexts: ${usedByContextsIds.join(', ')}`,
          error: true,
        });
        setShowActionDialog(false);
        setShowStatusModal(true);
        return;
      }
    }

    const response = await apiClient(showServerDownPopup)
      .node()
      .uninstallApplication(selectedAppId);
    if (response.error) {
      setUninstallStatus({
        title: 'Error',
        message: response.error.message,
        error: true,
      });
    } else {
      await setApps();
      setUninstallStatus({
        title: 'Success',
        message: 'Application uninstalled successfully',
        error: false,
      });
    }
    setShowActionDialog(false);
    setShowStatusModal(true);
  };

  const showModal = (id: string) => {
    setSelectedAppId(id);
    setShowActionDialog(true);
  };

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
          uninstallApplication={uninstallApplication}
          showStatusModal={showStatusModal}
          closeModal={() => setShowStatusModal(false)}
          uninstallStatus={uninstallStatus}
          showActionDialog={showActionDialog}
          setShowActionDialog={setShowActionDialog}
          showModal={showModal}
          errorMessage={errorMessage}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
