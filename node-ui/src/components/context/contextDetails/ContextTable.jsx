import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../../constants/en.global.json";
import { ContentCard } from "../../common/ConentCard";
import OptionsHeader from "../../../components/common/OptionsHeader";
import ListTable from "../../common/ListTable";
import rowItem from "../RowItem";
import { DetailsOptions } from "../../../constants/ContextConstants";

const FlexWrapper = styled.div`
  flex: 1;
`;

export default function ContextTable({
  nodeContextDetails,
  naviageToContextList,
  currentOption,
  setCurrentOption,
  tableOptions,
}) {
  const t = translations.contextPage;

  return (
    <ContentCard
    headerBackText="Context Details"
    headerOnBackClick={naviageToContextList}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={true}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        {currentOption == DetailsOptions.DETAILS && <div>DETAILS</div>}
        {currentOption == DetailsOptions.CLIENT_KEYS && (
          <ListTable
            ListDescription={t.invitedListDescription}
            columnItems={2}
            ListItems={nodeContextDetails.clientKeys}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noInviedAppsListText}
          />
        )}
        {currentOption == DetailsOptions.USERS && (
          <ListTable
            ListDescription={t.invitedListDescription}
            columnItems={2}
            ListItems={nodeContextDetails.users}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noInviedAppsListText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}

ContextTable.propTypes = {
  nodeContextDetails: PropTypes.object.isRequired,
  naviageToContextList: PropTypes.func.isRequired,
  currentOption: PropTypes.string.isRequired,
  setCurrentOption: PropTypes.func.isRequired,
  tableOptions: PropTypes.array.isRequired,
};
