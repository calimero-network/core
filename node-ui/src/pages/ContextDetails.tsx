import React, { useState, useEffect, useCallback } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import ContextTable from '../components/context/contextDetails/ContextTable';
import { useParams, useNavigate } from 'react-router-dom';
import { apiClient } from '@calimero-network/calimero-client';
import { DetailsOptions } from '../constants/ContextConstants';
import { useRPC } from '../hooks/useNear';
import { TableOptions } from '../components/common/OptionsHeader';
import { ContextDetails } from '../types/context';
import { parseAppMetadata } from '../utils/metadata';
import { Context, ContextClientKey, ContextStorage } from '@calimero-network/calimero-client/lib/api/nodeApi';
import { User } from '@calimero-network/calimero-client/lib/api/contractApi';

const initialOptions = [
  {
    name: 'Details',
    id: DetailsOptions.DETAILS,
    count: -1,
  } as TableOptions,
  {
    name: 'Client Keys',
    id: DetailsOptions.CLIENT_KEYS,
    count: 0,
  } as TableOptions,
  {
    name: 'Users',
    id: DetailsOptions.USERS,
    count: 0,
  } as TableOptions,
];

export default function ContextDetailsPage() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [contextDetails, setContextDetails] = useState<ContextDetails>();
  const [contextDetailsError, setContextDetailsError] = useState<string | null>(
    null,
  );
  const [contextClientKeys, setContextClientKeys] = useState<ContextClientKey[]>();
  const [contextClientKeysError, setContextClientKeysError] = useState<
    string | null
  >(null);
  const [contextUsers, setContextUsers] = useState<string[]>();
  const [contextUsersError, setContextUsersError] = useState<string | null>(
    null,
  );
  const [contextStorage, setContextStorage] = useState<ContextStorage>();
  const [contextStorageError, setContextStorageError] = useState<string | null>(
    null,
  );
  const [currentOption, setCurrentOption] = useState<string>(
    DetailsOptions.DETAILS,
  );
  const [tableOptions, setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const { getPackage, getLatestRelease } = useRPC();

  const generateContextObjects = useCallback(
    async (context: Context, id: string, metadata?: number[]) => {
      let appId = context.applicationId;
      let packageData = null;
      let versionData = null;
      if (metadata && metadata.length !== 0) {
        appId =
          parseAppMetadata(metadata)?.contractAppId ?? context.applicationId;
        packageData = await getPackage(appId);
        versionData = await getLatestRelease(appId);
      }

      const contextDetails: ContextDetails = {
        applicationId: appId,
        contextId: id,
        package: packageData,
        release: versionData,
      };

      return contextDetails;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [id],
  );

  useEffect(() => {
    const fetchNodeContexts = async () => {
      if (id) {
        const [
          nodeContext,
          contextClientKeys,
          contextClientUsers,
          contextStorage,
        ] = await Promise.all([
          apiClient.node().getContext(id),
          apiClient.node().getContextClientKeys(id),
          apiClient.node().getContextUsers(id),
          apiClient.node().getContextStorageUsage(id),
        ]);

        if (nodeContext.data) {
          const applicationMetadata = (
            await apiClient.node().getInstalledApplicationDetails(nodeContext.data.applicationId)
          ).data?.metadata;
          const contextObject = await generateContextObjects(
            nodeContext.data,
            id,
            applicationMetadata,
          );
          setContextDetails(contextObject);
        } else {
          setContextDetailsError(nodeContext.error?.message);
        }

        if (contextClientKeys.data) {
          setContextClientKeys(contextClientKeys.data.clientKeys);
        } else {
          setContextClientKeysError(contextClientKeys.error?.message);
        }

        if (contextClientUsers.data) {
          setContextUsers(contextClientUsers.data.identities);
        } else {
          setContextUsersError(contextClientUsers.error?.message);
        }

        if (contextStorage.data) {
          setContextStorage(contextStorage.data);
        } else {
          setContextStorageError(contextStorage.error?.message);
        }

        setTableOptions([
          {
            name: 'Details',
            id: DetailsOptions.DETAILS,
            count: -1,
          },
          {
            name: 'Client Keys',
            id: DetailsOptions.CLIENT_KEYS,
            count: contextClientKeys.data?.clientKeys?.length ?? 0,
          },
          {
            name: 'Users',
            id: DetailsOptions.USERS,
            count: contextClientUsers.data?.identities?.length ?? 0,
          },
        ]);
      }
    };
    fetchNodeContexts();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generateContextObjects, id]);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        {contextDetails &&
          contextClientKeys &&
          contextUsers &&
          contextStorage && (
            <ContextTable
              contextDetails={contextDetails}
              contextDetailsError={contextDetailsError}
              contextClientKeys={contextClientKeys}
              contextClientKeysError={contextClientKeysError}
              contextUsers={contextUsers}
              contextUsersError={contextUsersError}
              contextStorage={contextStorage}
              contextStorageError={contextStorageError}
              navigateToContextList={() => navigate('/contexts')}
              currentOption={currentOption}
              setCurrentOption={setCurrentOption}
              tableOptions={tableOptions}
            />
          )}
      </PageContentWrapper>
    </FlexLayout>
  );
}
