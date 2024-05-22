import React from "react";
import { useNavigate } from "react-router-dom";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";

export default function ConfirmWallet() {
  const navigate = useNavigate();
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
      }}
    >
      <MetamaskContext
        applicationId={"admin-ui"}
        rpcBaseUrl={"http://localhost:2428"}
        successRedirect={() => console.log("root key added")}
        navigateBack={() => navigate("/login")}
        clientLogin={false}
      />
    </div>
  );
}
