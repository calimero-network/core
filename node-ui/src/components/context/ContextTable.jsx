import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import OptionsHeader from "../../components/common/OptionsHeader";
import ListTable from "../common/ListTable";
import rowItem from "./RowItem";
import { Options } from "../../constants/ContextConstants";

const FlexWrapper = styled.div`
  flex: 1;
`;

export default function ContextTable({
  nodeContextList,
  naviageToStartContext,
  currentOption,
  setCurrentOption,
  tableOptions,
}) {
  const t = translations.contextPage;

  return (
    <ContentCard
      headerTitle={t.contextPageTitle}
      headerOptionText={t.startNewContextText}
      headerOnOptionClick={naviageToStartContext}
      headerDescription={t.contextPageDescription}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={true}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        {currentOption == Options.JOINED ? (
          <ListTable
            ListDescription={t.joinedListDescription}
            ListHeaderItems={["ID", "Installed Applications"]}
            columnItems={2}
            ListItems={nodeContextList.joined}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noJoinedAppsListText}
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

ContextTable.propTypes = {
  nodeContextList: PropTypes.object.isRequired,
  naviageToStartContext: PropTypes.func.isRequired,
  currentOption: PropTypes.string.isRequired,
  setCurrentOption: PropTypes.func.isRequired,
  tableOptions: PropTypes.array.isRequired,
};
