import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../../constants/en.global.json";
import { ContentCard } from "../../common/ConentCard";
import OptionsHeader from "../../../components/common/OptionsHeader";
import ListTable from "../../common/ListTable";
import rowItem from "./RowItem";
import { Options } from "../../../constants/ApplicationsConstants";
import { XMarkIcon } from "@heroicons/react/24/solid";

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

export default function ApplicationsTable({
  applicationsList,
  currentOption,
  setCurrentOption,
  tableOptions,
  closeModal,
  selectApplication,
}) {
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
          <ListTable
            ListHeaderItems={headersList}
            columnItems={4}
            ListItems={applicationsList.available}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noAvailableAppsText}
            onRowItemClick={selectApplication}
          />
        ) : (
          <ListTable
            ListHeaderItems={headersList}
            columnItems={4}
            ListItems={applicationsList.owned}
            rowItem={rowItem}
            roundedTopList={true}
            noItemsText={t.noOwnedAppsText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}

ApplicationsTable.propTypes = {
  applicationsList: PropTypes.object.isRequired,
  currentOption: PropTypes.string.isRequired,
  setCurrentOption: PropTypes.func.isRequired,
  tableOptions: PropTypes.array.isRequired,
  closeModal: PropTypes.func.isRequired,
  selectApplication: PropTypes.func.isRequired,
};
