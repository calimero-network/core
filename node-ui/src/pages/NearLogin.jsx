import React from "react";
import { useNavigate } from "react-router-dom";
import CenteredWrapper from "../components/common/CenteredPageWrapper";
import { WalletSelectorContextProvider } from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/WalletSelectorContext";
import NearLogin from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/NearLogin";
import { config } from "../utils/nearConfig";

export default function Near() {
  const navigate = useNavigate();
  return (
    <CenteredWrapper>
      <WalletSelectorContextProvider network={"testnet"}>
        <NearLogin
          appId={config.applicationId}
          rpcBaseUrl={config.nodeServerUrl}
          successRedirect={() => navigate("/identity")}
          navigateBack={() => navigate("/login")}
        />
      </WalletSelectorContextProvider>
    </CenteredWrapper>
  );
}
