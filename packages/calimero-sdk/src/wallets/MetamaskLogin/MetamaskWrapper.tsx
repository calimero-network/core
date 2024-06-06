import React from "react";
import LoginWithMetamask from "./LoginWithMetamask";
import MetamaskRootKey from "./MetamaskRootKey";
import { MetaMaskUIProvider } from "@metamask/sdk-react-ui";

interface MetamaskWrapperProps {
  applicationId: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  cardBackgroundColor?: string;
  metamaskTitleColor?: string;
  navigateBack?: () => void;
  clientLogin?: boolean;
}

const MetamaskWrapper: React.FC<MetamaskWrapperProps> = ({
  applicationId,
  rpcBaseUrl,
  successRedirect,
  cardBackgroundColor,
  metamaskTitleColor,
  navigateBack,
  clientLogin = true,
}) => {
  return (
    <MetaMaskUIProvider
      sdkOptions={{
        dappMetadata: {
          name: applicationId,
        },
        checkInstallationOnAllCalls: true,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          backgroundColor: cardBackgroundColor ?? "#1C1C1C",
          padding: "2rem",
          gap: "1rem",
          borderRadius: "0.5rem",
          width: "fit-content",
        }}
      >
        <div>
          {clientLogin ? (
            <LoginWithMetamask
              applicationId={applicationId}
              rpcBaseUrl={rpcBaseUrl}
              successRedirect={successRedirect}
              metamaskTitleColor={metamaskTitleColor}
              navigateBack={navigateBack}
            />
          ) : (
            <MetamaskRootKey
              applicationId={applicationId}
              rpcBaseUrl={rpcBaseUrl}
              successRedirect={successRedirect}
              metamaskTitleColor={metamaskTitleColor}
              navigateBack={navigateBack}
            />
          )}
        </div>
      </div>
    </MetaMaskUIProvider>
  );
};

export default MetamaskWrapper;
