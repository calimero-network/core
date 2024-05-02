import React from "react";
import { useNavigate } from "react-router-dom";
import { WalletSelectorContextProvider } from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/WalletSelectorContext";
import NearLogin from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/NearLogin/NearLogin";

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
          appId={"9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP"}
          rpcBaseUrl={"http://localhost:2428"}
          successRedirect={() => navigate("/")}
          navigateBack={() => navigate("/login")}
        />
      </WalletSelectorContextProvider>
    </div>
  );
}
