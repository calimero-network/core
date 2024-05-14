import React from "react";
import { ContentLayout } from "../applications/ApplicationsContent";
import { IdentityTable } from "./IdentityTable";

interface IdentityContentProps {
  identityList: any[];
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
