import React from 'react';
import styled from 'styled-components';
import translations from '../../../constants/en.global.json';
import ListTable from '../../common/ListTable';
import { AppDetails } from '../../../pages/ApplicationDetails';
import { Release } from '../../../pages/Applications';
import DetailsCard from './DetailsCard';
import releaseRowItem from './ReleaseRowItem';
import { ContentCard } from '../../common/ContentCard';

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ApplicationDetailsTableProps {
  applicationInformation: AppDetails;
  navigateToApplicationList: () => void;
  navigateToAddRelease: () => void;
}

export default function ApplicationDetailsTable({
  applicationInformation,
  navigateToApplicationList,
  navigateToAddRelease,
}: ApplicationDetailsTableProps) {
  const t = translations.applicationsPage.applicationsTable;

  return (
    <ContentCard
      headerBackText={'Application'}
      headerOnBackClick={navigateToApplicationList}
      descriptionComponent={
        <DetailsCard details={applicationInformation.package} />
      }
      headerOptionText="Add new release"
      headerOnOptionClick={() => {
        //TODO remove button to upload release for local apps
        if (applicationInformation.package.owner.length > 0) {
          navigateToAddRelease();
        }
      }}
    >
      <FlexWrapper>
        <ListTable<Release>
          numOfColumns={4}
          listHeaderItems={['VERSION', 'PATH', 'NOTES', 'HASH']}
          listItems={applicationInformation.releases || []}
          rowItem={releaseRowItem}
          roundTopItem={true}
          noItemsText={t.noAvailableReleasesText}
        />
      </FlexWrapper>
    </ContentCard>
  );
}
