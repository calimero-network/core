import React, { useState } from "react";
import axios from "axios";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import styled from "styled-components";
import ExportCard from "../components/exportIdentity/ExportCard";
import translations from "../constants/en.global.json";

const ExportWrapper = styled.div`
  display: flex;
  width: 100%;
  padding: 2rem;
  gap: 1rem;
`;
export default function Export() {
  const t = translations.exportIdentityPage;
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [exportStatus, setExportStatus] = useState({
    title: "",
    data: "",
    error: false,
  });

  const exportIdentity = async () => {
    try {
      const response = await axios.get("/admin-api/did");
      const identity = JSON.stringify(response?.data?.data?.root_keys, null, 2);
      setExportStatus({
        title: t.exportSuccessTitle,
        data: identity,
        error: false,
      });
    } catch (error) {
      console.error("Error exporting identity", error);
      setExportStatus({
        title: t.exportErrorTitle,
        data: error.message,
        error: true,
      });
    }
    setShowStatusModal(true);
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!exportStatus.error) {
      navigator.clipboard.writeText(exportStatus.data);
    }
    setExportStatus({
      title: "",
      data: "",
      error: false,
    });
  };

  return (
    <FlexLayout>
      <Navigation />
      <ExportWrapper>
        <ExportCard
          onClick={exportIdentity}
          showStatusModal={showStatusModal}
          closeStatusModal={closeStatusModal}
          exportStatus={exportStatus}
        />
      </ExportWrapper>
    </FlexLayout>
  );
}
