import React, { useState, useEffect, useCallback } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import ContextTable from '../components/context/ContextTable';
import { ContextOptions } from '../constants/ContextConstants';
import { useNavigate } from 'react-router-dom';
import { useRPC } from '../hooks/useNear';
import apiClient from '../api/index';
import {
  Context,
  ContextList,
  ContextsList,
} from '../api/dataSource/NodeDataSource';
import { ModalContent } from '../components/common/StatusModal';
import { TableOptions } from '../components/common/OptionsHeader';
import { ResponseData } from '../api/response';
import { ContextObject } from '../types/context';
import { useServerDown } from '../context/ServerDownContext';

const initialOptions = [
  {
    name: 'Joined',
    id: ContextOptions.JOINED,
    count: 0,
  } as TableOptions,
];

export default function ContextsPage() {
  const navigate = useNavigate();
  const { showServerDownPopup } = useServerDown();
  const { getPackage } = useRPC();
  const [currentOption, setCurrentOption] = useState<string>(
    ContextOptions.JOINED,
  );
  const [tableOptions, setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const [errorMessage, setErrorMessage] = useState('');
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [showActionDialog, setShowActionDialog] = useState(false);
  const [selectedContextId, setSelectedContextId] = useState<string | null>(
    null,
  );
  const [deleteStatus, setDeleteStatus] = useState<ModalContent>({
    title: '',
    message: '',
    error: false,
  });
  const [nodeContextList, setNodeContextList] = useState<
    ContextsList<ContextObject>
  >({
    joined: [],
  });

  const generateContextObjects = useCallback(
    async (contexts: Context[]): Promise<ContextObject[]> => {
      try {
        const tempContextObjects: ContextObject[] = await Promise.all(
          contexts.map(async (app: Context) => {
            const packageData = await getPackage(app.applicationId);
            const contextObject: ContextObject = {
              id: app.id,
              package: packageData,
            };
            return contextObject;
          }),
        );
        return tempContextObjects;
      } catch (error) {
        console.error('Error generating context objects:', error);
        return [];
      }
    },
    [getPackage],
  );

  const fetchNodeContexts = useCallback(async () => {
    setErrorMessage('');
    const fetchContextsResponse: ResponseData<ContextList> = await apiClient(
      showServerDownPopup,
    )
      .node()
      .getContexts();
    // TODO - fetch invitations
    if (fetchContextsResponse.error) {
      setErrorMessage(fetchContextsResponse.error.message);
      return;
    }
    if (fetchContextsResponse.data) {
      const nodeContexts = fetchContextsResponse.data;
      const joinedContexts = await generateContextObjects(
        nodeContexts.contexts,
      );

      setNodeContextList((prevState: ContextsList<ContextObject>) => ({
        ...prevState,
        joined: joinedContexts,
      }));
      setTableOptions([
        {
          name: 'Joined',
          id: ContextOptions.JOINED,
          count: nodeContexts.contexts?.length ?? 0,
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    fetchNodeContexts();
  }, [fetchNodeContexts]);

  const deleteNodeContext = async () => {
    if (!selectedContextId) return;
    const deleteContextResponse = await apiClient(showServerDownPopup)
      .node()
      .deleteContext(selectedContextId);
    if (deleteContextResponse.error) {
      setDeleteStatus({
        title: 'Error',
        message: `Could not delete context with id: ${selectedContextId}!`,
        error: true,
      });
    } else {
      setDeleteStatus({
        title: 'Success',
        message: `Context with id: ${selectedContextId} deleted.`,
        error: false,
      });
    }
    setSelectedContextId(null);
    setShowActionDialog(false);
    setShowStatusModal(true);
  };

  const closeStatusModal = async () => {
    setShowStatusModal(false);
    if (!deleteStatus.error) {
      await fetchNodeContexts();
    }
    setDeleteStatus({
      title: '',
      message: '',
      error: false,
    });
  };

  const showModal = (id: string) => {
    setSelectedContextId(id);
    setShowActionDialog(true);
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ContextTable
          nodeContextList={nodeContextList}
          navigateToStartContext={() => navigate('/contexts/start-context')}
          navigateToJoinContext={() => navigate('/contexts/join-context')}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
          deleteNodeContext={deleteNodeContext}
          showStatusModal={showStatusModal}
          closeModal={closeStatusModal}
          deleteStatus={deleteStatus}
          showActionDialog={showActionDialog}
          setShowActionDialog={setShowActionDialog}
          showModal={showModal}
          errorMessage={errorMessage}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
