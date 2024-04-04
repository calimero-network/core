import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { IdentityContent } from "../components/identity/IdentityContent";

export default function Identity() {
  return (
    <FlexLayout>
      <Navigation />
      <IdentityContent
        identityList={[]}
        deleteIdentity={() => console.log("del")}
        addIdentity={() => console.log("add")}
      />
    </FlexLayout>
  );
}
