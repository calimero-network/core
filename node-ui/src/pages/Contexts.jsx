import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import PageContentWrapper from "../components/common/PageContentWrapper";
import ContextTable from "../components/context/ContextTable";
import apiClient from "../api/index";

export default function Contexts() {
  const [nodeContextList, setNodeContextList] = useState([]);

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
        <ContextTable nodeContextList={nodeContextList} />
      </PageContentWrapper>
    </FlexLayout>
  );
}
