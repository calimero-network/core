import React from "react";
import { useNavigate } from "react-router-dom";
import { WalletSelectorContextProvider } from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/WalletSelectorContext";
import NearRootKey from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/NearRootKey";

import "@near-wallet-selector/modal-ui/styles.css";

export default function Bootstrap() {
  const navigate = useNavigate();
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
        backgroundColor: "#111111",
      }}
    >
      <WalletSelectorContextProvider network={"testnet"}>
        <NearRootKey
          appId={"admin-ui"}
          rpcBaseUrl={"http://localhost:2428"}
          successRedirect={() => navigate("/")}
          navigateBack={() => navigate("/login")}
          cardBackgroundColor={"#1c1c1c"}
          nearTitleColor={"white"}
        />
      </WalletSelectorContextProvider>
    </div>
  );
}
