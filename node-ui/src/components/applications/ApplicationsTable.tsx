import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ContentCard";
import OptionsHeader, { TableOptions } from "../common/OptionsHeader";
import ListTable from "../common/ListTable";
import applicationRowItem from "./ApplicationRowItem";
import { Options } from "../../constants/ApplicationsConstants";
import { Application, Applications } from "../../pages/Applications";
import { AddNewItem } from "../common/AddNewItem";

const FlexWrapper = styled.div`
  flex: 1;
  position: relative;

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

  .install-app-wrapper {
    display: flex;
    width: 100%;
    justify-content: center;
    position: absolute;
    bottom: -1.75rem;
  }
`;

interface ApplicationsTableProps {
  applicationsList: Applications;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: TableOptions[];
  navigateToAppDetails: (applicationId: string) => void;
  navigateToPublishApp: () => void;
}

export default function ApplicationsTable({
  applicationsList,
  currentOption,
  setCurrentOption,
  tableOptions,
  navigateToAppDetails,
  navigateToPublishApp,
}: ApplicationsTableProps) {
  const t = translations.applicationsPage.applicationsTable;
  const headersList = ["NAME", "ID", "LATEST VERSION", "PUBLISHED"];

  return (
    <ContentCard
      headerTitle={t.title}
      headerOptionText={t.publishNewAppText}
      headerOnOptionClick={navigateToPublishApp}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={false}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        {currentOption == Options.AVAILABLE ? (
          <ListTable<Application>
            listHeaderItems={headersList}
            numOfColumns={4}
            listItems={applicationsList.available}
            rowItem={applicationRowItem}
            roundTopItem={true}
            noItemsText={t.noAvailableAppsText}
            onRowItemClick={navigateToAppDetails}
          />
        ) : (
          <ListTable<Application>
            listHeaderItems={headersList}
            numOfColumns={4}
            listItems={applicationsList.owned}
            rowItem={applicationRowItem}
            roundTopItem={true}
            noItemsText={t.noOwnedAppsText}
            onRowItemClick={navigateToAppDetails}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
