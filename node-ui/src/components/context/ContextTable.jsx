import React, { useState } from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import { Content } from "../../constants/ContextConstants";
import OptionsHeader from "../../components/common/OptionsHeader";
import ListTable from "../common/ListTable";
import rowItem from "./RowItem";

const FlexWrapper = styled.div`
  flex: 1;
`;

const Options = {
  JOINED: "JOINED",
  INVITED: "INVITED"
}

export default function ContextTable({
  pageContent,
  nodeContextList,
  switchContent,
}) {
  const t = translations.contextPage;
  const [tableOptions, _setTableOptions] = useState([
    {
      name: "Joined",
      id: Options.JOINED,
      count: 0,
    },
    {
      name: "Invited",
      id: Options.INVITED,
      count: 0,
    },
  ]);
  const [currentOption, setCurrentOption] = useState(Options.JOINED);

  return (
    <>
      {pageContent === Content.CONTEXT_LIST ? (
        <ContentCard
          headerTitle={t.contextPageTitle}
          headerOptionText={t.startNewContextText}
          headerOnOptionClick={() => switchContent(Content.START_NEW_CONTEXT)}
          headerDescription={t.contextPageDescription}
        >
          <FlexWrapper>
            <OptionsHeader
              tableOptions={tableOptions}
              showOptionsCount={true}
              currentOption={currentOption}
              setCurrentOption={setCurrentOption}
            />
            {currentOption == Options.JOINED ? <ListTable
              ListDescription={"Contexts the node is running"}
              ListHeaderItems={["ID", "Installed Applications"]}
              columnItems={2}
              ListItems={[]}
              rowItem={rowItem}
              roundedTopList={true}
            /> : <div>Invited</div>}
          </FlexWrapper>
        </ContentCard>
      ) : (
        <ContentCard
          headerBackText={t.startNewContextText}
          headerOnBackClick={() => switchContent(Content.CONTEXT_LIST)}
        ></ContentCard>
      )}
    </>
  );
}

ContextTable.propTypes = {
  nodeContextList: PropTypes.array.isRequired,
  pageContent: PropTypes.string.isRequired,
  switchContent: PropTypes.func.isRequired,
};
