import React, { Fragment, useCallback, useEffect, useState } from "react";
import { randomBytes } from "crypto";
import { providers } from "near-api-js";
import type { AccountView } from "near-api-js/lib/providers/provider";
import {
  verifyFullKeyBelongsToUser,
  verifySignature,
  type SignedMessage,
  type SignMessageParams,
} from "@near-wallet-selector/core";

import { useWalletSelector } from "./WalletSelectorContext";
import { getOrCreateKeypair } from "../crypto/ed25519";
import apiClient from "../api";
import { ResponseData } from "../api-response";
import { setStorageNodeAuthorized } from "../storage/storage";
import { Loading } from "./loading/Loading";
import {
  LoginRequest,
  NearSignatureMessageMetadata,
  NodeChallenge,
  Payload,
  SignatureMessage,
  SignatureMessageMetadata,
  WalletMetadata,
  WalletSignatureData,
  WalletType,
} from "../nodeApi";

import "@near-wallet-selector/modal-ui/styles.css";

export interface Message {
  premium: boolean;
  sender: string;
  text: string;
}

export type Account = AccountView & {
  account_id: string;
};

interface NearLoginProps {
  rpcBaseUrl: string;
  appId: string;
}

const NearLogin: React.FC<NearLoginProps> = ({ rpcBaseUrl, appId }) => {
  const { selector, accounts, modal, accountId } = useWalletSelector();
  const [_account, setAccount] = useState<Account | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const appName = "me";

  const getAccount = useCallback(async (): Promise<Account | null> => {
    if (!accountId) {
      return null;
    }

    const { network } = selector.options;
    const provider = new providers.JsonRpcProvider({ url: network.nodeUrl });

    return provider
      .query<AccountView>({
        request_type: "view_account",
        finality: "final",
        account_id: accountId,
      })
      .then((data: any) => ({
        ...data,
        account_id: accountId,
      }));
  }, [accountId, selector]);

  useEffect(() => {
    const timeoutId = setTimeout(() => {
      verifyMessageBrowserWallet();
    }, 500);

    return () => {
      clearTimeout(timeoutId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!accountId) {
      return setAccount(null);
    }

    setLoading(true);

    getAccount().then((nextAccount: any) => {
      setAccount(nextAccount);
      setLoading(false);
    });
  }, [accountId, getAccount]);

  async function handleSignOut() {
    const wallet = await selector.wallet();

    wallet.signOut().catch((err: any) => {
      console.log("Failed to sign out");
      console.error(err);
    });
  }

  function handleSwitchWallet() {
    modal.show();
  }

  function handleSwitchAccount() {
    const currentIndex = accounts.findIndex((x) => x.accountId === accountId);
    const nextIndex = currentIndex < accounts.length - 1 ? currentIndex + 1 : 0;

    const nextAccountId = accounts[nextIndex].accountId;

    selector.setActiveAccount(nextAccountId);

    alert("Switched account to " + nextAccountId);
  }

  const verifyMessage = useCallback(
    async (
      message: SignMessageParams,
      signedMessage: SignedMessage
    ): Promise<boolean> => {
      console.log("verifyMessage", { message, signedMessage });

      const verifiedSignature = verifySignature({
        message: message.message,
        nonce: message.nonce,
        recipient: message.recipient,
        publicKey: signedMessage.publicKey,
        signature: signedMessage.signature,
        callbackUrl: message.callbackUrl,
      });
      const verifiedFullKeyBelongsToUser = await verifyFullKeyBelongsToUser({
        publicKey: signedMessage.publicKey,
        accountId: signedMessage.accountId,
        network: selector.options.network,
      });

      const isMessageVerified =
        verifiedFullKeyBelongsToUser && verifiedSignature;

      const resultMessage = isMessageVerified
        ? "Successfully verified"
        : "Failed to verify";

      console.log(
        `${resultMessage} signed message: '${
          message.message
        }': \n ${JSON.stringify(signedMessage)}`
      );

      return isMessageVerified;
    },
    [selector.options.network]
  );

  const verifyMessageBrowserWallet = useCallback(async () => {
    const urlParams = new URLSearchParams(
      window.location.hash.substring(1) // skip the first char (#)
    );
    const accId = urlParams.get("accountId") as string;
    const publicKey = urlParams.get("publicKey") as string;
    const signature = urlParams.get("signature") as string;

    if (!accId && !publicKey && !signature) {
      console.error("Missing params in url.");
      return;
    }

    const message: SignMessageParams = JSON.parse(
      localStorage.getItem("message")!
    );

    const state: SignatureMessageMetadata = JSON.parse(message.state!);

    const signedMessage = {
      accountId: accId,
      publicKey,
      signature,
    };

    const isMessageVerified: boolean = await verifyMessage(
      message,
      signedMessage
    );

    const url = new URL(location.href);
    url.hash = "";
    url.search = "";
    window.history.replaceState({}, document.title, url);
    localStorage.removeItem("message");
    // eslint-disable-next-line react-hooks/exhaustive-deps

    if (isMessageVerified) {
      const signatureMetadata: NearSignatureMessageMetadata = {
        recipient: message.recipient,
        callbackUrl: message.callbackUrl!,
        nonce: message.nonce.toString("base64"),
      };
      const payload: Payload = {
        message: state,
        metadata: signatureMetadata,
      };
      const walletSignatureData: WalletSignatureData = {
        payload: payload,
        clientPubKey: publicKey,
      };
      const walletMetadata: WalletMetadata = {
        type: WalletType.NEAR,
        signingKey: publicKey,
      };
      const loginRequest: LoginRequest = {
        walletSignature: signature,
        payload: walletSignatureData.payload!,
        walletMetadata: walletMetadata,
      };

      await apiClient
        .node()
        .login(loginRequest, rpcBaseUrl)
        .then((result) => {
          console.log("result", result);
          if (result.error) {
            console.error("login error", result.error);
            //TODO handle error
          } else {
            setStorageNodeAuthorized();
            console.log("login success");
          }
        })
        .catch(() => {
          console.error("error while login");
          //TODO handle error
        });
    } else {
      //TODO handle error
      console.error("Message not verified");
    }
  }, [verifyMessage]);

  async function handleSignMessage() {
    const challengeResponseData: ResponseData<NodeChallenge> = await apiClient
      .node()
      .requestChallenge(rpcBaseUrl, appId);
    const { publicKey } = await getOrCreateKeypair();

    if (challengeResponseData.error) {
      console.log("requestChallenge api error", challengeResponseData.error);
      return;
    }

    const wallet = await selector.wallet("my-near-wallet");

    const challengeNonce =
      challengeResponseData?.data?.nonce ?? randomBytes(32).toString("hex");

    const nonce: Buffer = Buffer.from(challengeNonce, "base64");
    const recipient = appName;
    const callbackUrl = location.href;
    const applicationId = challengeResponseData.data?.applicationId ?? "";
    const nodeSignature = challengeResponseData.data?.nodeSignature ?? "";
    const timestamp =
      challengeResponseData.data?.timestamp ?? new Date().getTime();

    const signatureMessage: SignatureMessage = {
      nodeSignature,
      clientPublicKey: publicKey,
    };
    const message: string = JSON.stringify(signatureMessage);

    const state: SignatureMessageMetadata = {
      clientPublicKey: publicKey,
      nodeSignature,
      nonce: nonce.toString("base64"),
      applicationId,
      timestamp,
      message,
    };

    if (wallet.type === "browser") {
      console.log("browser");

      localStorage.setItem(
        "message",
        JSON.stringify({
          message,
          nonce: [...nonce],
          recipient,
          callbackUrl,
          state: JSON.stringify(state),
        })
      );
    }

    await wallet.signMessage({ message, nonce, recipient, callbackUrl });
  }

  if (loading) {
    return <Loading />;
  }

  return (
    <Fragment>
      <div style={{ display: "flex", flexDirection: "column" }}>
        {accountId && <div style={{ textAlign: "center" }}>
          Account Id: <span style={{ color: "#FF7A00" }}>{accountId}</span>
        </div>}
        <div style={{ display: "flex", marginTop: "1.5rem" }}>
          <button
            style={{ marginRight: "0.5rem" }}
            onClick={() => {
              if (accountId) {
                handleSignOut();
              }
            }}
          >
            Sign Out
          </button>
          <button
            style={{ marginRight: "0.5rem" }}
            onClick={handleSwitchWallet}
          >
            Switch Wallet
          </button>
          <button style={{ marginRight: "0.5rem" }} onClick={handleSignMessage}>
            Authenticate
          </button>
          {accounts.length > 1 && (
            <button onClick={handleSwitchAccount}>Switch Account</button>
          )}
        </div>
      </div>
    </Fragment>
  );
};

export default NearLogin;
