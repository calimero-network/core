import React from "react";
import LoginWithMetamask from "./Metamask";
import { MetaMaskUIProvider } from "@metamask/sdk-react-ui";

interface MetamaskContextProps {
  applicationId: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  cardBackgroundColor: string | undefined;
  metamaskTitleColor: string | undefined;
  navigateBack: () => void | undefined;
}

const MetamaskContext: React.FC<MetamaskContextProps> = ({
  applicationId,
  rpcBaseUrl,
  successRedirect,
  cardBackgroundColor,
  metamaskTitleColor,
  navigateBack
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
          <LoginWithMetamask
            applicationId={applicationId}
            rpcBaseUrl={rpcBaseUrl}
            successRedirect={successRedirect}
            metamaskTitleColor={metamaskTitleColor}
            navigateBack={navigateBack}
          />
        </div>
      </div>
    </MetaMaskUIProvider>
  );
};

export default MetamaskContext;
