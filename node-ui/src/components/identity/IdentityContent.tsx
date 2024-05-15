import React from "react";
import { ContentLayout } from "../applications/ApplicationsContent";
import { IdentityTable } from "./IdentityTable";
import { RootKey } from "src/pages/Identity";

interface IdentityContentProps {
  identityList: RootKey[];
  deleteIdentity: (id: number) => void;
  addIdentity: () => void;
}

export function IdentityContent({ identityList, deleteIdentity, addIdentity }: IdentityContentProps) {
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="page-title">Identity</div>
        <IdentityTable
          identityList={identityList}
          deleteIdentity={deleteIdentity}
          addIdentity={addIdentity}
        />
      </div>
    </ContentLayout>
  );
}
