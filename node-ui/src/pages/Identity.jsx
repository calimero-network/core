import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { IdentityContent } from "../components/identity/IdentityContent";

export default function Identity() {
  return (
    <FlexLayout>
      <Navigation />
      <IdentityContent
        identityList={[
          {
            id: "did:cali:12D3KooWK2kSHzeTE5daWqFtqN2GMoUNFFN6Lps8zz47hHy5hAhQ",
            verificationMethod: [
              {
                id: "did:cali:12D3KooWK2kSHzeTE5daWqFtqN2GMoUNFFN6Lps8zz47hHy5hAhQ#key1",
                type: "Ed25519",
                publicKeyMultibase:
                  "zCovLVG4fQcqSmSqox5oVUdK4pvMsuNxbZLLs6gwwBrqqjFdz3QFGTsgtopjX91ek2gk2SkQ",
                controller:
                  "did:cali:12D3KooWK2kSHzeTE5daWqFtqN2GMoUNFFN6Lps8zz47hHy5hAhQ",
              },
            ],
          },
        ]}
        deleteIdentity={() => console.log("del")}
        addIdentity={() => console.log("add")}
      />
    </FlexLayout>
  );
}
