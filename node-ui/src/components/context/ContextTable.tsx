import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { ContentCard } from '../common/ContentCard';
import OptionsHeader, { TableOptions } from '../common/OptionsHeader';
import ListTable from '../common/ListTable';
import rowItem from './RowItem';
import StatusModal, { ModalContent } from '../common/StatusModal';
import ActionDialog from '../common/ActionDialog';
import { ContextsList } from '../../api/dataSource/NodeDataSource';
import { ContextObject } from '../../types/context';

console.log('testign');

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ContextTableProps {
  nodeContextList: ContextsList<ContextObject>;
  navigateToStartContext: () => void;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: TableOptions[];
  deleteNodeContext: () => void;
  showStatusModal: boolean;
  closeModal: () => void;
  deleteStatus: ModalContent;
  showActionDialog: boolean;
  setShowActionDialog: (show: boolean) => void;
  showModal: (id: string) => void;
  errorMessage: string;
}

export default function ContextTable({
  nodeContextList,
  navigateToStartContext,
  currentOption,
  setCurrentOption,
  tableOptions,
  deleteNodeContext,
  showStatusModal,
  closeModal,
  deleteStatus,
  showActionDialog,
  setShowActionDialog,
  showModal,
  errorMessage,
}: ContextTableProps) {
  const t = translations.contextPage;
  console.log('testing');
  return (
    <ContentCard
      headerTitle={t.contextPageTitle}
      headerOptionText={t.startNewContextText}
      headerDescription={t.contextPageDescription}
      headerOnOptionClick={navigateToStartContext}
    >
      <div>testing</div>
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={deleteStatus}
      />
      <ActionDialog
        show={showActionDialog}
        closeDialog={() => setShowActionDialog(false)}
        onConfirm={deleteNodeContext}
        title={t.actionDialog.title}
        subtitle={t.actionDialog.subtitle}
      />
      <FlexWrapper>
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={true}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        <ListTable<ContextObject>
          listHeaderItems={['ID', 'INSTALLED APPLICATION']}
          numOfColumns={2}
          listItems={nodeContextList.joined}
          rowItem={rowItem}
          roundTopItem={true}
          noItemsText={t.noJoinedAppsListText}
          onRowItemClick={showModal}
          error={errorMessage}
        />
      </FlexWrapper>
    </ContentCard>
  );
}
