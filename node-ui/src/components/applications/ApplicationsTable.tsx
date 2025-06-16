import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { ContentCard } from '../common/ContentCard';
import OptionsHeader, { TableOptions } from '../common/OptionsHeader';
import ListTable from '../common/ListTable';
import applicationRowItem from './ApplicationRowItem';
import { Options } from '../../constants/ApplicationsConstants';
import { Applications, Application } from '../../pages/Applications';
import installedApplicationRowItem from './InstalledApplicationRowItem';
import StatusModal, { ModalContent } from '../common/StatusModal';
import ActionDialog from '../common/ActionDialog';

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
  uninstallApplication: () => void;
  showStatusModal: boolean;
  closeModal: () => void;
  uninstallStatus: ModalContent;
  showActionDialog: boolean;
  setShowActionDialog: (show: boolean) => void;
  showModal: (id: string) => void;
  errorMessage: string;
}

export default function ApplicationsTable(props: ApplicationsTableProps) {
  const t = translations.applicationsPage.applicationsTable;
  const headersList = ['NAME', 'ID', 'LATEST VERSION', 'PUBLISHED BY'];

  return (
    <ContentCard
      headerTitle={t.title}
      headerOptionText={t.publishNewAppText}
      headerOnOptionClick={props.navigateToPublishApp}
      headerSecondOptionText={t.installNewAppText}
      headerOnSecondOptionClick={props.navigateToInstallApp}
    >
      <StatusModal
        show={props.showStatusModal}
        closeModal={props.closeModal}
        modalContent={props.uninstallStatus}
      />
      <ActionDialog
        show={props.showActionDialog}
        closeDialog={() => props.setShowActionDialog(false)}
        onConfirm={props.uninstallApplication}
        title={t.actionDialog.title}
        subtitle={t.actionDialog.subtitle}
        buttonActionText={t.actionDialog.buttonActionText}
      />
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
            numOfColumns={5}
            listItems={props.applicationsList.installed}
            rowItem={installedApplicationRowItem}
            roundTopItem={true}
            noItemsText={t.noInstalledAppsText}
            onRowItemClick={(applicationId: string) =>
              props.showModal(applicationId)
            }
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
