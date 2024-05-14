import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import OptionsHeader from "../common/OptionsHeader";
import ListTable from "../common/ListTable";
import rowItem from "./RowItem";
import { Options } from "../../constants/ContextConstants";
import StatusModal from "../common/StatusModal";
import ActionDialog from "../common/ActionDialog";

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ContextTableProps {
  nodeContextList: any;
  naviageToStartContext: () => void;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: any;
  deleteNodeContext: (contextId: string) => void;
  showStatusModal: boolean;
  closeModal: () => void;
  deleteStatus: any;
  showActionDialog: boolean;
  setShowActionDialog: (show: boolean) => void;
  showModal: (contextId: string) => void;
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
          <ListTable
            ListHeaderItems={["ID", "INSTALLED APPLICATION"]}
            columnItems={2}
            ListItems={nodeContextList.joined}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noJoinedAppsListText}
            onRowItemClick={showModal}
          />
        ) : (
          <ListTable
            ListDescription={t.invitedListDescription}
            columnItems={2}
            ListItems={nodeContextList.invited}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noInviedAppsListText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
