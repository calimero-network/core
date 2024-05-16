import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import OptionsHeader from "../common/OptionsHeader";
import ListTable from "../common/ListTable";
import rowItem from "./RowItem";
import { Options } from "../../constants/ContextConstants";
import StatusModal, { ModalContent } from "../common/StatusModal";
import ActionDialog from "../common/ActionDialog";
import { ContextsList } from "../../api/dataSource/NodeDataSource";
import { ContextObject, TableOptions } from "../../pages/Contexts";

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ContextTableProps {
  nodeContextList: ContextsList<ContextObject>;
  naviageToStartContext: () => void;
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
}

export default function ContextTable({
  nodeContextList,
  naviageToStartContext,
  currentOption,
  setCurrentOption,
  tableOptions,
  deleteNodeContext,
  showStatusModal,
  closeModal,
  deleteStatus,
  showActionDialog,
  setShowActionDialog,
  showModal
}: ContextTableProps) {
  const t = translations.contextPage;

  return (
    <ContentCard
      headerTitle={t.contextPageTitle}
      headerOptionText={t.startNewContextText}
      headerOnOptionClick={naviageToStartContext}
      headerDescription={t.contextPageDescription}
    >
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
        {currentOption == Options.JOINED ? (
          <ListTable<ContextObject>
            ListHeaderItems={["ID", "INSTALLED APPLICATION"]}
            columnItems={2}
            ListItems={nodeContextList.joined}
            rowItem={rowItem}
            roundTopItem={true}
            noItemsText={t.noJoinedAppsListText}
            onRowItemClick={showModal}
          />
        ) : (
          <ListTable<ContextObject>
            ListDescription={t.invitedListDescription}
            columnItems={2}
            ListItems={nodeContextList.invited}
            rowItem={rowItem}
            roundTopItem={true}
            noItemsText={t.noInviedAppsListText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
