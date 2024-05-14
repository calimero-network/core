import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/contextDetails/ContextTable";
import { useParams } from "react-router-dom";
import apiClient from "../api/index";
import { DetailsOptions } from "../constants/ContextConstants";
import { useNavigate } from "react-router-dom";
import { useRPC } from "../hooks/useNear";

const initialOptions = [
  {
    name: "Details",
    id: DetailsOptions.DETAILS,
    count: -1,
  },
  {
    name: "Client Keys",
    id: DetailsOptions.CLIENT_KEYS,
    count: 0,
  },
  {
    name: "Users",
    id: DetailsOptions.USERS,
    count: 0,
  },
];

interface ContextObject {
  id: string;
  applicationId: string;
  name: string;
  description: string;
  version: string;
  created: string;
  updated: string;
  owner: string;
  clientKeys: string[];
  users: string[];
}

export default function ContextDetails() {
  const { id } = useParams();
  const navigate = useNavigate();
  const [nodeContextDetails, setNodeContextDetails] = useState<ContextObject>();
  const [currentOption, setCurrentOption] = useState(DetailsOptions.DETAILS);
  const [tableOptions, setTableOptions] = useState(initialOptions);
  const { getPackage, getLatestRelease } = useRPC();

  const generateContextObjects = async (context: any) => {
    let tempContextObjects: ContextObject = {
      id: "",
      applicationId: "",
      name: "",
      description: "",
      version: "",
      created: "",
      updated: "",
      owner: "",
      clientKeys: [],
      users: []
    };
    const packageData = await getPackage(context.applicationId);
    const versionData = await getLatestRelease(context.applicationId);
    tempContextObjects = {
      ...packageData,
      ...context,
      ...versionData
    };
    return tempContextObjects;
  };

  useEffect(() => {
    const fetchNodeContexts = async () => {
      const nodeContext = await apiClient.context().getContext(id);
      if (nodeContext) {
        const contextObject = await generateContextObjects(nodeContext);
        setNodeContextDetails(contextObject);
        //TBD - after client keys and users are implemented
        setTableOptions([
          {
            name: "Details",
            id: DetailsOptions.DETAILS,
            count: -1,
          },
          {
            name: "Client Keys",
            id: DetailsOptions.CLIENT_KEYS,
            count: 0,
          },
          {
            name: "Users",
            id: DetailsOptions.USERS,
            count: 0,
          },
        ]);
      }
    };
    fetchNodeContexts();
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
      <ContextTable
          nodeContextDetails={nodeContextDetails}
          naviageToContextList={() => navigate("/contexts")}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
