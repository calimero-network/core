import React, { useEffect, useState } from "react";
import axios from "axios";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { useNavigate } from "react-router-dom";
import PageContentWrapper from "../components/common/PageContentWrapper";
import IdentityTable from "../components/identity/IdentityTable";
import { RootKeyObject, mapApiResponseToObjects } from "../utils/rootkey";

export interface RootKey {
  signing_key: string;
}

export default function Identity() {
  const navigate = useNavigate();
  const [rootKeys, setRootKeys] = useState<RootKeyObject[]>([]);
  useEffect(() => {
    const setDids = async () => {
      const response = await axios.get("/admin-api/did");
      const rootKeyList = mapApiResponseToObjects(response.data.data.root_keys);
      setRootKeys(rootKeyList);
    };
    setDids();
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <IdentityTable
          onAddRootKey={() => navigate("/")}
          rootKeysList={rootKeys}
          onCopyKeyClick={(publicKey: string) => navigator.clipboard.writeText(publicKey)}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
