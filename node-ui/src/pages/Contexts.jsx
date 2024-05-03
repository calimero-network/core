import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/ContextTable";
import apiClient from "../api/index";
import { Content } from "../constants/ContextConstants";



export default function Contexts() {
  const [nodeContextList, setNodeContextList] = useState([]);
  const [pageContent, setPageContent] = useState(Content.CONTEXT_LIST);

  useEffect(() => {
    const fetchNodeContexts = async () => {
      const nodeContexts = await apiClient.context().getContexts();
      if (nodeContexts.length !== 0) {
        setNodeContextList(nodeContexts);
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
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
