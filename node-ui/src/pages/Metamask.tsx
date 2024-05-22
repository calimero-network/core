import React from "react";
import { useNavigate } from "react-router-dom";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";
import ContentWrapper from "../components/login/ContentWrapper";

export default function Metamask() {
  const navigate = useNavigate();
  return (
    <ContentWrapper>
      <MetamaskContext
        applicationId={"admin-ui"}
        rpcBaseUrl={window.location.origin}
        successRedirect={() => navigate("/identity")}
        navigateBack={() => navigate("/")}
        clientLogin={false}
      />
    </ContentWrapper>
  );
}
