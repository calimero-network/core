import React from "react";
import LoginSelector from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/LoginSelector";
import { useNavigate } from "react-router-dom";
import CenteredWrapper from "../components/common/CenteredPageWrapper";

export default function Login() {
  const navigate = useNavigate();
  return (
    <CenteredWrapper>
      <LoginSelector
        navigateMetamaskLogin={() => navigate("/metamask")}
        navigateNearLogin={() => navigate("/near")}
      />
    </CenteredWrapper>
  );
}
