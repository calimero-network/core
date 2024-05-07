import React from "react";
import { useNavigate } from "react-router-dom";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";
import { config } from "../utils/nearConfig";

export default function MetamaskLogin() {
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
        applicationId={"app-id"}
        rpcBaseUrl={"http://localhost:2428"}
        successRedirect={() => navigate("/")}
        navigateBack={() => navigate("/login")}
      />
    </div>
  );
}
