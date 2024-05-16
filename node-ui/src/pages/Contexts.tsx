import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/ContextTable";
import { Options } from "../constants/ContextConstants";
import { useNavigate } from "react-router-dom";
import { useRPC } from "../hooks/useNear";
import apiClient from "../api/index";
import { Context, ContextsList } from "../api/dataSource/NodeDataSource";
import { ModalContent } from "../components/common/StatusModal";
import { TableOptions } from "../components/common/OptionsHeader";

export interface ContextObject {
  id: string;
  applicationId: string;
  name: string;
  description: string;
  repository: string;
  owner: string;
}

const initialOptions = [
  {
    name: "Joined",
    id: Options.JOINED,
    count: 0,
  },
  {
    name: "Invited",
    id: Options.INVITED,
    count: 0,
  },
];

export default function Contexts() {
  const navigate = useNavigate();
  const { getPackage } = useRPC();
  const [currentOption, setCurrentOption] = useState<string>(Options.JOINED);
  const [tableOptions, setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [showActionDialog, setShowActionDialog] = useState(false);
  const [selectedContextId, setSelectedContextId] = useState<string | null>(
    null
  );
  const [deleteStatus, setDeleteStatus] = useState<ModalContent>({
    title: "",
    message: "",
    error: false,
  });
  const [nodeContextList, setNodeContextList] = useState<
    ContextsList<ContextObject>
  >({
    joined: [],
    invited: [],
  });

  const generateContextObjects = async (
    contexts: Context[]
  ): Promise<ContextObject[]> => {
    try {
      const tempContextObjects = await Promise.all(
        contexts.map(async (app: Context) => {
          const packageData = await getPackage(app.applicationId);
          return { ...packageData, id: app.id, applicationId: packageData.id };
        })
      );
      return tempContextObjects;
    } catch (error) {
      console.error("Error generating context objects:", error);
      return [];
    }
  };

  const fetchNodeContexts = async () => {
    const nodeContexts = await apiClient.node().getContexts();
    if (nodeContexts) {
      const joinedContexts = await generateContextObjects(nodeContexts.joined);

      setNodeContextList((prevState: ContextsList<ContextObject>) => ({
        ...prevState,
        joined: joinedContexts,
      }));
      setTableOptions([
        {
          name: "Joined",
          id: Options.JOINED,
          count: nodeContexts.joined?.length ?? 0,
        },
        {
          name: "Invited",
          id: Options.INVITED,
          count: nodeContexts.invited?.length ?? 0,
        },
      ]);
    }
  };

  useEffect(() => {
    fetchNodeContexts();
  }, []);

  const deleteNodeContext = async () => {
    if (!selectedContextId) return;
    const nodeContexts = await apiClient
      .node()
      .deleteContext(selectedContextId);
    if (nodeContexts) {
      setDeleteStatus({
        title: "Success",
        message: `Context with id: ${selectedContextId} deleted.`,
        error: false,
      });
    } else {
      setDeleteStatus({
        title: "Error",
        message: `Could not delete context with id: ${selectedContextId}!`,
        error: true,
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
      title: "",
      message: "",
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
          naviageToStartContext={() => navigate("/contexts/start-context")}
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
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
