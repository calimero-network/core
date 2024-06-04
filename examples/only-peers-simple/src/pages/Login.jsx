import React from "react";
import LoginSelector from "@calimero-is-near/calimero-p2p-sdk/lib/wallets/LoginSelector";
import { useNavigate } from "react-router-dom";

export default function Login() {
  const navigate = useNavigate();
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
      }}
    >
      <LoginSelector
        navigateMetamaskLogin={() => navigate("/metamask")}
        navigateNearLogin={() => navigate("/near")}
      />
    </div>
  );
}
