import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import ListTable from "../common/ListTable";
import { RootKeyObject } from "../../utils/rootkey";
import IdentityRowItem from "./IdentityRowItem";

const FlexWrapper = styled.div`
  flex: 1;
`;

interface IdentitiesTableProps {
  rootKeysList: RootKeyObject[];
  onAddRootKey: () => void;
  onCopyKeyClick: (publicKey: string) => void;
}

export default function IdentitiesTable({
  rootKeysList,
  onAddRootKey,
  onCopyKeyClick
}: IdentitiesTableProps) {
  const t = translations.identityPage;
  return (
    <ContentCard
      headerTitle={t.title}
      headerOptionText={t.addRootKeyText}
      headerOnOptionClick={onAddRootKey}
      headerDescription={
        rootKeysList.length > 0 ?
        `${t.loggedInLabel}${rootKeysList[0]!.publicKey}` : ""
      }
    >
      <FlexWrapper>
        <ListTable<RootKeyObject>
          listHeaderItems={["TYPE", "ADDED", "PUBLIC KEY"]}
          numOfColumns={6}
          listItems={rootKeysList}
          rowItem={IdentityRowItem}
          roundTopItem={true}
          noItemsText={t.noRootKeysText}
          onRowItemClick={onCopyKeyClick}
        />
      </FlexWrapper>
    </ContentCard>
  );
}