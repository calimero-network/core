import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/ContextTable";
import apiClient from "../api/index";
import { Options } from "../constants/ContextConstants";
import { useNavigate } from "react-router-dom";

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
]

export default function Contexts() {
  const navigate = useNavigate();
  const [nodeContextList, setNodeContextList] = useState({ joined: [], invited: [] });
  const [currentOption, setCurrentOption] = useState(Options.JOINED);
  const [tableOptions, setTableOptions] = useState(initialOptions);

  useEffect(() => {
    const fetchNodeContexts = async () => {
      const nodeContexts = await apiClient.context().getContexts();
      if (nodeContexts) {
        setNodeContextList(nodeContexts);
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
    fetchNodeContexts();
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ContextTable
          nodeContextList={nodeContextList}
          naviageToStartContext={() => navigate("/start-context")}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
