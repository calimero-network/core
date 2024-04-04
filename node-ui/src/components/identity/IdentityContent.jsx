import React from "react";
import { ContentLayout } from "../applications/ApplicationsContent";
import { IdentityTable } from "./IdentityTable";
import PropTypes from "prop-types";

export function IdentityContent({ identityList, deleteIdentity, addIdentity }) {
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

IdentityContent.propTypes = {
  identityList: PropTypes.array.isRequired,
  deleteIdentity: PropTypes.func.isRequired,
  addIdentity: PropTypes.func.isRequired,
};
