import React from "react";
import { useNavigate } from "react-router-dom";
import MetamaskContext from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/MetamaskLogin/MetamaskWrapper";

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
        applicationId={"9SFTEoc6RBHtCn9b6cm4PPmhYzrogaMCd5CRiYAQichP"}
        rpcBaseUrl={"http://localhost:2428"}
        successRedirect={() => navigate("/")}
        navigateBack={() => navigate("/login")}
      />
    </div>
  );
}
