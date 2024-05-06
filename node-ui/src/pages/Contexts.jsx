import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/ContextTable";
import apiClient from "../api/index";
import { Content, Options } from "../constants/ContextConstants";

export default function Contexts() {
  const [nodeContextList, setNodeContextList] = useState({ joined: [], invited: [] });
  const [pageContent, setPageContent] = useState(Content.CONTEXT_LIST);
  const [currentOption, setCurrentOption] = useState(Options.JOINED);
  const [tableOptions, setTableOptions] = useState([
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
  ]);

  useEffect(() => {
    const fetchNodeContexts = async () => {
      const nodeContexts = await apiClient.context().getContexts();
      if (nodeContexts.length !== 0) {
        setNodeContextList(nodeContexts);
        setTableOptions([
          {
            name: "Joined",
            id: Options.JOINED,
            count: nodeContexts.joined.length,
          },
          {
            name: "Invited",
            id: Options.INVITED,
            count: nodeContexts.invited.length,
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
          pageContent={pageContent}
          switchContent={(content) => setPageContent(content)}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
