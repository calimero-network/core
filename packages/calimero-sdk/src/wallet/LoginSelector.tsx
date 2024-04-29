import React, { useState } from "react";
import MetamaskIcon from "./MetamaskLogin/MetamaskIcon";
import NearIcon from "./NearLogin/NearIcon";
import MetamaskContext from "./MetamaskLogin/MetamaskWrapper";
import { WalletSelectorContextProvider } from "./NearLogin/WalletSelectorContext";
import { NetworkId } from "@near-wallet-selector/core";
import NearLogin from "./NearLogin/NearLogin";

interface LoginSelectorProps {
  applicationId: string;
  rpcBaseUrl: string;
  successRedirect: () => void | undefined;
  cardBackgroundColor: string | undefined;
  metamaskTitleColor: string | undefined;
  nearTitleColor: string | undefined;
  network: NetworkId;
}

enum WalletType {
  METAMASK = "METAMASK",
  NEAR = "NEAR",
}

const LoginSelector: React.FC<LoginSelectorProps> = ({
  applicationId,
  rpcBaseUrl,
  successRedirect,
  cardBackgroundColor,
  metamaskTitleColor,
  nearTitleColor,
  network,
}) => {
  const [walletSelected, setWalletSelected] = useState<WalletType | null>(null);
  return (
    <div
     
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          backgroundColor: cardBackgroundColor ?? "#1C1C1C",
          gap: "1rem",
          borderRadius: "0.5rem",
          width: "fit-content",
        }}
    >
      {walletSelected === null && (
        <div
          style={{
            padding: "2rem",
          }}
        >
          <div
            style={{
              width: "100%",
              textAlign: "center",
              color: "white",
              marginTop: "6px",
              marginBottom: "6px",
              fontSize: "1.5rem",
              lineHeight: "2rem",
              fontWeight: "medium",
            }}
          >
            Continue with wallet
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              width: "100%",
              gap: "8px",
              paddingTop: "50px",
            }}
          >
            <button
              style={{
                width: "100%",
                display: "flex",
                justifyContent: "center",
                alignItems: "center",
                gap: "2px",
                height: "46px",
                cursor: "pointer",
                fontSize: "1rem",
                lineheight: "1.5rem",
                fontWeight: "500",
                lineHeight: "1.25rem",
                borderRadius: "0.375rem",
                backgroundColor: "#FF7A00",
                color: "white",
                border: "none",
                outline: "none",
              }}
              onClick={() => setWalletSelected(WalletType.METAMASK)}
            >
              <MetamaskIcon />
              <span>Metamask wallet</span>
            </button>
            <button
              style={{
                width: "100%",
                display: "flex",
                justifyContent: "center",
                alignItems: "center",
                gap: "2px",
                height: "46px",
                cursor: "pointer",
                fontSize: "1rem",
                lineheight: "1.5rem",
                fontWeight: "500",
                lineHeight: "1.25rem",
                borderRadius: "0.375rem",
                backgroundColor: "#D1D5DB",
                color: "black",
                border: "none",
                outline: "none",
              }}
              onClick={() => setWalletSelected(WalletType.NEAR)}
            >
              <NearIcon />
              <span>Near wallet</span>
            </button>
          </div>
        </div>
      )}
      {walletSelected === WalletType.METAMASK && (
        <MetamaskContext
          applicationId={applicationId}
          rpcBaseUrl={rpcBaseUrl}
          successRedirect={successRedirect}
          navigateBack={() => setWalletSelected(null)}
          cardBackgroundColor={cardBackgroundColor}
          metamaskTitleColor={metamaskTitleColor}
        />
      )}
      {walletSelected === WalletType.NEAR && (
        <WalletSelectorContextProvider network={network}>
          <NearLogin
            appId={applicationId}
            rpcBaseUrl={rpcBaseUrl}
            successRedirect={successRedirect}
            navigateBack={() => setWalletSelected(null)}
            cardBackgroundColor={cardBackgroundColor}
            nearTitleColor={nearTitleColor}
          />
        </WalletSelectorContextProvider>
      )}
    </div>
  );
};

export default LoginSelector;
