import React, { useCallback, useEffect, useState } from "react";
import {
  MetaMaskButton,
  useAccount,
  useSDK,
  // @ts-ignore: sdk-react-ui does not export useSignMessage
  useSignMessage,
} from "@metamask/sdk-react-ui";
import apiClient from "../../api";
import {
  NodeChallenge,
  RootKeyRequest,
  WalletType,
} from "../../nodeApi";
import { ResponseData } from "../../api-response";
import { setStorageNodeAuthorized } from "../../storage/storage";
import { Loading } from "../loading/Loading";

interface MetamaskRootKeyProps {
  applicationId: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  metamaskTitleColor: string | undefined;
  navigateBack: () => void | undefined;
}

export default function MetamaskRootKey({
  applicationId,
  rpcBaseUrl,
  successRedirect,
  metamaskTitleColor,
  navigateBack,
}: MetamaskRootKeyProps) {
  const { isConnected, address } = useAccount();
  const [walletSignatureData, setWalletSignatureData] =
    useState(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const { ready } = useSDK();

  const signatureMessage = useCallback((): string | undefined => {
    return walletSignatureData
      ? walletSignatureData
      : undefined;
  }, [walletSignatureData]);

  const {
    data: signData,
    isError: isSignError,
    isLoading: isSignLoading,
    isSuccess: isSignSuccess,
    signMessage,
  } = useSignMessage({
    message: signatureMessage(),
  });

  const requestNodeData = useCallback(async () => {
    const challengeResponseData: ResponseData<NodeChallenge> = await apiClient
      .node()
      .requestChallenge(rpcBaseUrl, applicationId);
    setWalletSignatureData(challengeResponseData.data?.nodeSignature ?? "");
  }, []);

  const login = useCallback(async () => {
    setErrorMessage(null);
    if (!signData) {
      console.error("signature is empty");
      //TODO handle error
    } else if (!address) {
      console.error("address is empty");
      //TODO handle error
    } else {
      const rootKeyRequest: RootKeyRequest = {
        accountId: address,
        signature: signData,
        publicKey: address,
        callbackUrl: "",
        message: walletSignatureData,
        walletMetadata: {
          type: WalletType.ETH,
          signingKey: address,
        },
      }
      await apiClient
        .node()
        .addRootKey(rootKeyRequest, rpcBaseUrl)
        .then((result) => {
          if (result.error) {
            console.error("Login error: ", result.error);
            setErrorMessage(result.error.message);
          } else {
            setStorageNodeAuthorized();
            successRedirect();
          }
        })
        .catch(() => {
          console.error("error while login!");
          setErrorMessage("Error while login!");
        });
    }
  }, [address, signData, walletSignatureData?.payload]);

  useEffect(() => {
    if (isConnected) {
      requestNodeData();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isConnected]);

  useEffect(() => {
    if (isSignSuccess && walletSignatureData) {
      //send request to node
      console.log("signature", signData);
      console.log("address", address);
      login();
    }
  }, [address, isSignSuccess, login, signData, walletSignatureData]);

  if (!ready) {
    return <Loading />;
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        padding: "0.5rem",
      }}
    >
      <div
        style={{
          marginTop: "1.5rem",
          display: "grid",
          color: "white",
          fontSize: "1.25rem",
          fontWeight: "500",
          textAlign: "center",
        }}
      >
        <span
          style={{
            marginBottom: "0.5rem",
            color: metamaskTitleColor ?? "#fff",
          }}
        >
          Metamask
        </span>
        <header
          style={{
            marginTop: "1.5rem",
            display: "flex",
            flexDirection: "column",
          }}
        >
          <MetaMaskButton
            theme="dark"
            color={isConnected && walletSignatureData ? "blue" : "white"}
            buttonStyle={
              isConnected && walletSignatureData
                ? {
                    display: "flex",
                    justifyContent: "center",
                    alignItems: "center",
                    backgroundColor: "#25282D",
                    height: "73px",
                    borderRadius: "6px",
                    border: "none",
                    outline: "none",
                  }
                : {
                    cursor: "pointer",
                  }
            }
          ></MetaMaskButton>
          {isConnected && walletSignatureData && (
            <div style={{ marginTop: "155px" }}>
              <button
                style={{
                  backgroundColor: "#FF7A00",
                  color: "white",
                  width: "100%",
                  display: "flex",
                  justifyContent: "center",
                  alignItems: "center",
                  gap: "0.5rem",
                  height: "46px",
                  cursor: "pointer",
                  fontSize: "1rem",
                  fontWeight: "500",
                  borderRadius: "0.375rem",
                  border: "none",
                  outline: "none",
                  paddingLeft: "0.5rem",
                  paddingRight: "0.5rem",
                }}
                disabled={isSignLoading}
                onClick={() => signMessage()}
              >
                Add root key
              </button>
              {isSignError && (
                <div
                  style={{
                    color: "red",
                    fontSize: "14px",
                    fontWeight: "500",
                    marginTop: "0.5rem",
                  }}
                >
                  Error signing message
                </div>
              )}
              <div
                style={{
                  color: "red",
                  fontSize: "14px",
                  fontWeight: "500",
                  marginTop: "0.5rem",
                }}
              >
                {errorMessage}
              </div>
            </div>
          )}
        </header>
      </div>
      <div
        style={{
          paddingTop: "1rem",
          fontSize: "14px",
          color: "#fff",
          textAlign: "center",
          cursor: "pointer",
        }}
        onClick={navigateBack}
      >
        Back to wallet selector
      </div>
    </div>
  );
}
