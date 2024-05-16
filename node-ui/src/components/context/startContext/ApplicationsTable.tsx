import React from "react";
import styled from "styled-components";
import translations from "../../../constants/en.global.json";
import { ContentCard } from "../../common/ConentCard";
import OptionsHeader from "../../common/OptionsHeader";
import ListTable from "../../common/ListTable";
import rowItem from "./RowItem";
import { Options } from "../../../constants/ApplicationsConstants";
import { XMarkIcon } from "@heroicons/react/24/solid";
import { Applications } from "./ApplicationsPopup";
import { Application } from "../../../pages/Applications";

const FlexWrapper = styled.div`
  flex: 1;

  .close-button {
    position: absolute;
    right: 0.875rem;
    top: 0.875rem;
    cursor: pointer;
    color: #fff;
    height: 1.5rem;
    width: 1.5rem;

    &:hover {
      color: #4cfafc;
    }
  }
`;

interface ApplicationsTableProps {
  applicationsList: Applications;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: any[];
  closeModal: () => void;
  selectApplication: (applicationId: string) => void;
}

export default function ApplicationsTable({
  applicationsList,
  currentOption,
  setCurrentOption,
  tableOptions,
  closeModal,
  selectApplication,
}: ApplicationsTableProps) {
  const t = translations.startContextPage.applicationList;
  const headersList = ["NAME", "ID", "LATEST VERSION", "PUBLISHED"];

  return (
    <ContentCard headerTitle={t.listTitle}>
      <FlexWrapper>
        <XMarkIcon onClick={closeModal} className="close-button" />
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={true}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        {currentOption == Options.AVAILABLE ? (
          <ListTable<Application>
            listHeaderItems={headersList}
            columnItems={4}
            listItems={applicationsList.available}
            rowItem={rowItem}
            roundTopItem={true}
            noItemsText={t.noAvailableAppsText}
            onRowItemClick={selectApplication}
          />
        ) : (
          <ListTable<Application>
            listHeaderItems={headersList}
            columnItems={4}
            listItems={applicationsList.owned}
            rowItem={rowItem}
            roundTopItem={true}
            noItemsText={t.noOwnedAppsText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
