import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { ContentCard } from '../common/ContentCard';
import OptionsHeader, { TableOptions } from '../common/OptionsHeader';
import ListTable from '../common/ListTable';
import applicationRowItem from './ApplicationRowItem';
import { Options } from '../../constants/ApplicationsConstants';
import { Applications } from '../../pages/Applications';
import { Application } from '../../api/dataSource/NodeDataSource';

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
`;

interface ApplicationsTableProps {
  applicationsList: Applications;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: TableOptions[];
  navigateToAppDetails: (app: Application | undefined) => void;
  navigateToPublishApp: () => void;
  navigateToInstallApp: () => void;
  changeSelectedTab: (option: string) => void;
  errorMessage: string;
}

export default function ApplicationsTable(props: ApplicationsTableProps) {
  const t = translations.applicationsPage.applicationsTable;
  const headersList = ['NAME', 'ID', 'LATEST VERSION', 'PUBLISHED'];

  return (
    <ContentCard
      headerTitle={t.title}
      headerOptionText={t.publishNewAppText}
      headerOnOptionClick={props.navigateToPublishApp}
      headerSecondOptionText={t.installNewAppText}
      headerOnSecondOptionClick={props.navigateToInstallApp}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={props.tableOptions}
          showOptionsCount={false}
          currentOption={props.currentOption}
          setCurrentOption={props.setCurrentOption}
        />
        {props.currentOption === Options.AVAILABLE && (
          <ListTable<Application>
            listHeaderItems={headersList}
            numOfColumns={4}
            listItems={props.applicationsList.available}
            rowItem={applicationRowItem}
            roundTopItem={true}
            noItemsText={t.noAvailableAppsText}
            onRowItemClick={(applicationId: string) => {
              var app = props.applicationsList.available.find(
                (app) => app.id === applicationId,
              );
              props.navigateToAppDetails(app);
            }}
            error={props.errorMessage}
          />
        )}
        {props.currentOption === Options.OWNED && (
          <ListTable<Application>
            listHeaderItems={headersList}
            numOfColumns={4}
            listItems={props.applicationsList.owned}
            rowItem={applicationRowItem}
            roundTopItem={true}
            noItemsText={t.noOwnedAppsText}
            onRowItemClick={(applicationId: string) => {
              var app = props.applicationsList.owned.find(
                (app) => app.id === applicationId,
              );
              props.navigateToAppDetails(app);
            }}
            error={props.errorMessage}
          />
        )}
        {props.currentOption === Options.INSTALLED && (
          <ListTable<Application>
            listHeaderItems={headersList}
            numOfColumns={4}
            listItems={props.applicationsList.installed}
            rowItem={applicationRowItem}
            roundTopItem={true}
            noItemsText={t.noInstalledAppsText}
            onRowItemClick={(applicationId: string) => {
              var app = props.applicationsList.installed.find(
                (app) => app.id === applicationId,
              );
              props.navigateToAppDetails(app);
            }}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
