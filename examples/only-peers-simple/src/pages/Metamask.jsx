import React from "react";
import { useNavigate } from "react-router-dom";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";
import { config } from "../calimeroConfig";

export default function Metamask() {
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
        applicationId={config.applicationId}
        rpcBaseUrl={config.nodeServerUrl}
        successRedirect={() => navigate("/")}
        navigateBack={() => navigate("/login")}
      />
    </div>
  );
}
