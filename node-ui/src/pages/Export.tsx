import React, { useState } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import styled from 'styled-components';
import ExportCard from '../components/exportIdentity/ExportCard';
import translations from '../constants/en.global.json';
import { ModalContentItem } from '../components/common/StatusModalItem';
import { apiClient } from '@calimero-network/calimero-client';

const ExportWrapper = styled.div`
  display: flex;
  width: 100%;
  padding: 4.705rem 2rem 2rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: 'slnt' 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;
`;
export default function ExportPage() {
  const t = translations.exportIdentityPage;
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [exportStatus, setExportStatus] = useState<ModalContentItem>({
    title: '',
    data: '',
    error: false,
  });

  const exportIdentity = async () => {
    try {
      const response = await apiClient.node().getDidList();
      const identity = JSON.stringify(response?.data?.did, null, 2);
      setExportStatus({
        title: t.exportSuccessTitle,
        data: identity,
        error: false,
      });
    } catch (error) {
      console.error('Error exporting identity', error);
      setExportStatus({
        title: t.exportErrorTitle,
        // @ts-ignore
        // TODO add erorr type
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
      title: '',
      data: '',
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
