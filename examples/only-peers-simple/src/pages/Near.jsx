import React from "react";
import { useNavigate } from "react-router-dom";
import { WalletSelectorContextProvider } from "@calimero-is-near/calimero-p2p-sdk/lib/wallets/NearLogin/WalletSelectorContext";
import NearLogin from "@calimero-is-near/calimero-p2p-sdk/lib/wallets/NearLogin/NearLogin";
import { config } from "../calimeroConfig";

import "@near-wallet-selector/modal-ui/styles.css";

export default function Near() {
  const navigate = useNavigate();
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
      }}
    >
      <WalletSelectorContextProvider network={"testnet"}>
        <NearLogin
          appId={config.applicationId}
          rpcBaseUrl={config.nodeServerUrl}
          successRedirect={() => navigate("/")}
          navigateBack={() => navigate("/login")}
        />
      </WalletSelectorContextProvider>
    </div>
  );
}
