import React, { useState, useEffect, useCallback } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import ContextTable from '../components/context/contextDetails/ContextTable';
import { useParams, useNavigate } from 'react-router-dom';
import apiClient from '../api/index';
import { DetailsOptions } from '../constants/ContextConstants';
import { useRPC } from '../hooks/useNear';
import { TableOptions } from '../components/common/OptionsHeader';
import {
  ClientKey,
  Context,
  ContextStorage,
  User,
} from '../api/dataSource/NodeDataSource';
import { ContextDetails } from '../types/context';
import { useServerDown } from '../context/ServerDownContext';

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
  const { showServerDownPopup } = useServerDown();
  const navigate = useNavigate();
  const [contextDetails, setContextDetails] = useState<ContextDetails>();
  const [contextDetailsError, setContextDetailsError] = useState<string | null>(
    null,
  );
  const [contextClientKeys, setContextClientKeys] = useState<ClientKey[]>();
  const [contextClientKeysError, setContextClientKeysError] = useState<
    string | null
  >(null);
  const [contextUsers, setContextUsers] = useState<User[]>();
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
    async (context: Context, id: string) => {
      const packageData = await getPackage(context.applicationId);
      const versionData = await getLatestRelease(context.applicationId);

      const contextDetails: ContextDetails = {
        applicationId: context.applicationId,
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
          apiClient(showServerDownPopup).node().getContext(id),
          apiClient(showServerDownPopup).node().getContextClientKeys(id),
          apiClient(showServerDownPopup).node().getContextUsers(id),
          apiClient(showServerDownPopup).node().getContextStorageUsage(id),
        ]);

        if (nodeContext.data) {
          const contextObject = await generateContextObjects(
            nodeContext.data.context,
            id,
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
          setContextUsers(contextClientUsers.data.contextUsers);
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
            count: contextClientUsers.data?.contextUsers?.length ?? 0,
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
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
      <div>testing</div>
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
