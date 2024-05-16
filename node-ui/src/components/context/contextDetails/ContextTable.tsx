import React from "react";
import styled from "styled-components";
import translations from "../../../constants/en.global.json";
import { ContentCard } from "../../common/ConentCard";
import OptionsHeader from "../../common/OptionsHeader";
import ListTable from "../../common/ListTable";
import clientKeyRowItem from "./ClientKeyRowItem";
import userRowItem from "./UserRowItem";
import { DetailsOptions } from "../../../constants/ContextConstants";
import DetailsCard from "./DetailsCard";
import { TableOptions } from "../../../pages/Contexts";
import { ContextObject } from "../../../pages/ContextDetails";

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ContextTableProps {
  nodeContextDetails: ContextObject;
  naviageToContextList: () => void;
  currentOption: string;
  setCurrentOption: (option: string) => void;
  tableOptions: TableOptions[];
}

export default function ContextTable({
  nodeContextDetails,
  naviageToContextList,
  currentOption,
  setCurrentOption,
  tableOptions,
}: ContextTableProps) {
  const t = translations.contextPage.contextDetails;
  
  return (
    <ContentCard
    headerBackText={t.title}
    headerOnBackClick={naviageToContextList}
    >
      <FlexWrapper>
        <OptionsHeader
          tableOptions={tableOptions}
          showOptionsCount={true}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
        />
        {currentOption == DetailsOptions.DETAILS && <DetailsCard details={nodeContextDetails}/>}
        {currentOption == DetailsOptions.CLIENT_KEYS && (
          <ListTable
            ListDescription={t.clientKeysListDescription}
            columnItems={3}
            ListHeaderItems={["TYPE", "ADDED", "PUBLIC KEY"]}
            ListItems={nodeContextDetails.clientKeys || []}
            rowItem={clientKeyRowItem}
            roundedTopList={true}
            noItemsText={t.noClientKeysText}
          />
        )}
        {currentOption == DetailsOptions.USERS && (
          <ListTable
            columnItems={2}
            ListItems={nodeContextDetails.users || []}
            ListHeaderItems={["USER ID", "JOINED"]}
            rowItem={userRowItem}
            roundedTopList={true}
            noItemsText={t.noUsersText}
          />
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
