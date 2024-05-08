import React from "react";
import { useNavigate } from "react-router-dom";
import CenteredWrapper from "../components/common/CenteredPageWrapper";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";
import { config } from "../utils/nearConfig";

export default function MetamaskLogin() {
  const navigate = useNavigate();
  return (
    <CenteredWrapper>
      <MetamaskContext
        appId={config.applicationId}
        rpcBaseUrl={config.nodeServerUrl}
        successRedirect={() => navigate("/")}
        navigateBack={() => navigate("/login")}
      />
    </CenteredWrapper>
  );
}
