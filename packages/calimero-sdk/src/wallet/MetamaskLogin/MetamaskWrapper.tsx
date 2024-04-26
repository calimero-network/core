import React from "react";
import LoginWithMetamask from "./Metamask";
import { MetaMaskUIProvider } from "@metamask/sdk-react-ui";

interface MetamaskContextProps {
  applicationId: string;
  rpcBaseUrl: string;
}

const MetamaskContext: React.FC<MetamaskContextProps> = ({ applicationId, rpcBaseUrl }) => {
  return (
    <div
      style={{
        display: "flex",
        width: "100%",
        height: "100vh",
        justifyContent: "center",
      }}
    >
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
            width: "100%",
            height: "100vh",
            justifyContent: "center",
            backgroundColor: "#111111",
          }}
        >
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              justifyContent: "center",
              alignItems: "center",
            }}
          >
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                backgroundColor: "#1C1C1C",
                padding: "2rem",
                gap: "1rem",
                borderRadius: "0.5rem",
              }}
            >
              <div>
                <LoginWithMetamask  applicationId={applicationId} rpcBaseUrl={rpcBaseUrl}/>
              </div>
            </div>
          </div>
        </div>
      </MetaMaskUIProvider>
    </div>
  );
};

export default MetamaskContext;
