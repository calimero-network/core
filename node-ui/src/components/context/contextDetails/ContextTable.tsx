import React from 'react';
import styled from 'styled-components';
import translations from '../../../constants/en.global.json';
import { ContentCard } from '../../common/ContentCard';
import OptionsHeader, { TableOptions } from '../../common/OptionsHeader';
import ListTable from '../../common/ListTable';
import clientKeyRowItem from './ClientKeyRowItem';
import userRowItem from './UserRowItem';
import { DetailsOptions } from '../../../constants/ContextConstants';
import DetailsCard from './DetailsCard';
import {
  ClientKey,
  ContextStorage,
  User,
} from '../../../api/dataSource/NodeDataSource';
import { ContextDetails } from '../../../types/context';

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ContextTableProps {
  contextDetails: ContextDetails;
  contextDetailsError: string | null;
  contextClientKeys: ClientKey[];
  contextClientKeysError: string | null;
  contextUsers: User[];
  contextUsersError: string | null;
  contextStorage: ContextStorage;
  contextStorageError: string | null;
  navigateToContextList: () => void;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: TableOptions[];
}

export default function ContextTable(props: ContextTableProps) {
  const t = translations.contextPage.contextDetails;

  return (
    <ContentCard
      headerBackText={t.title}
      headerOnBackClick={props.navigateToContextList}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={props.tableOptions}
          showOptionsCount={true}
          currentOption={props.currentOption}
          setCurrentOption={props.setCurrentOption}
        />
        {props.currentOption === DetailsOptions.DETAILS && (
          <DetailsCard
            details={props.contextDetails}
            detailsErrror={props.contextDetailsError}
            contextStorage={props.contextStorage}
            contextStorageError={props.contextStorageError}
          />
        )}
        {props.currentOption === DetailsOptions.CLIENT_KEYS && (
          <ListTable<ClientKey>
            listDescription={t.clientKeysListDescription}
            numOfColumns={3}
            listHeaderItems={['TYPE', 'ADDED', 'PUBLIC KEY']}
            listItems={props.contextClientKeys || []}
            error={props.contextClientKeysError ?? ''}
            rowItem={clientKeyRowItem}
            roundTopItem={true}
            noItemsText={t.noClientKeysText}
          />
        )}
        {props.currentOption === DetailsOptions.USERS && (
          <ListTable<User>
            numOfColumns={1}
            listItems={props.contextUsers || []}
            error={props.contextUsersError ?? ''}
            listHeaderItems={['Identity']}
            rowItem={userRowItem}
            roundTopItem={true}
            noItemsText={t.noUsersText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
