import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { KeysContent } from "../components/keyManagement/KeysContent";
import { KeysTable } from "../components/keyManagement/KeysTable";

const KEY_OPTIONS_ENABLED = false;

export default function Keys() {
  return (
    <FlexLayout>
      <Navigation />
      <KeysContent>
        <KeysTable
          nodeKeys={[]}
          setActive={() => console.log("set active")}
          revokeKey={() => console.log("revoke key")}
          optionsEnabled={KEY_OPTIONS_ENABLED}
        />
      </KeysContent>
    </FlexLayout>
  );
}
