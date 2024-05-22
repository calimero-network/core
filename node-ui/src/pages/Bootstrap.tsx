import React from "react";
import LoginSelector from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/LoginSelector";
import { useNavigate } from "react-router-dom";
import ContentWrapper from "../components/login/ContentWrapper";

export default function Bootstrap() {
  const navigate = useNavigate();
  return (
    <ContentWrapper>
      <LoginSelector
        navigateMetamaskLogin={() => navigate("/metamask")}
        navigateNearLogin={() => navigate("/near")}
        cardBackgroundColor={undefined}
      />
    </ContentWrapper>
  );
}
