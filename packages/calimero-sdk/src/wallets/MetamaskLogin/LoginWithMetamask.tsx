import React, { useCallback, useEffect, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../../auth/ed25519';

import {
  useAccount,
  MetaMaskButton,
  useSDK,
  // @ts-ignore: sdk-react-ui does not export useSignMessage
  useSignMessage,
} from '@metamask/sdk-react-ui';

import apiClient from '../../api';
import {
  EthSignatureMessageMetadata,
  LoginRequest,
  NodeChallenge,
  Payload,
  SignatureMessage,
  SignatureMessageMetadata,
  WalletMetadata,
  WalletSignatureData,
} from '../../api/nodeApi';
import { ResponseData } from '../../types/api-response';
import {
  setExecutorPublicKey,
  setStorageNodeAuthorized,
} from '../../storage/storage';
import { Loading } from '../loading/Loading';
import { getNetworkType } from '../eth/type';

interface LoginWithMetamaskProps {
  contextId?: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  metamaskTitleColor: string | undefined;
  navigateBack: () => void | undefined;
}

export function LoginWithMetamask({
  contextId,
  rpcBaseUrl,
  successRedirect,
  metamaskTitleColor,
  navigateBack,
}: LoginWithMetamaskProps) {
  const { isConnected, address } = useAccount();
  const [walletSignatureData, setWalletSignatureData] =
    useState<WalletSignatureData | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const { chainId, ready } = useSDK();

  const signatureMessage = useCallback((): string | undefined => {
    return walletSignatureData
      ? walletSignatureData?.payload?.message.message
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
      .requestChallenge(rpcBaseUrl, contextId);
    const { publicKey } = await getOrCreateKeypair();

    if (challengeResponseData.error) {
      console.error('requestNodeData error', challengeResponseData.error);
      //TODO handle error
      return;
    }

    const signatureMessage: SignatureMessage = {
      nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
      publicKey: publicKey,
    };

    const signatureMessageMetadata: SignatureMessageMetadata = {
      nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
      publicKey: publicKey,
      nonce:
        challengeResponseData.data?.nonce ?? randomBytes(32).toString('hex'),
      contextId: challengeResponseData.data?.contextId ?? null,
      timestamp: challengeResponseData.data?.timestamp ?? new Date().getTime(),
      message: JSON.stringify(signatureMessage),
    };
    const signatureMetadata: EthSignatureMessageMetadata = {};
    const payload: Payload = {
      message: signatureMessageMetadata,
      metadata: signatureMetadata,
    };
    const wsd: WalletSignatureData = {
      payload,
      publicKey,
    };
    setWalletSignatureData(wsd);
  }, []);

  const login = useCallback(async () => {
    setErrorMessage(null);
    if (!signData) {
      console.error('signature is empty');
      //TODO handle error
    } else if (!address) {
      console.error('address is empty');
      //TODO handle error
    } else {
      const walletMetadata: WalletMetadata = {
        wallet: getNetworkType(chainId),
        signingKey: address,
      };
      const loginRequest: LoginRequest = {
        walletSignature: signData,
        payload: walletSignatureData?.payload,
        walletMetadata: walletMetadata,
        contextId,
      };
      try {
        const result = await apiClient.node().login(loginRequest, rpcBaseUrl);

        if (result.error) {
          console.error('Login error: ', result.error);
          setErrorMessage(result.error.message);
          return;
        }

        try {
          const identity = await apiClient
            .node()
            .getContextIdentity(rpcBaseUrl, contextId);
          if (
            !identity ||
            !identity.data ||
            !identity.data.contextIdentities ||
            identity.data.contextIdentities.length === 0
          ) {
            console.error('Login error: No context identities found');
            setErrorMessage('Error while login! No context identities found.');
          }

          setExecutorPublicKey(identity.data.contextIdentities[0]);
          setStorageNodeAuthorized();
          successRedirect();
        } catch (identityError) {
          console.error('Error fetching context identity: ', identityError);
          setErrorMessage('Error fetching context identity!');
        }
      } catch (loginError) {
        console.error('Login error: ', loginError);
        setErrorMessage('Error while login!');
      }
    }
  }, [
    address,
    chainId,
    rpcBaseUrl,
    signData,
    successRedirect,
    walletSignatureData?.payload,
  ]);

  useEffect(() => {
    if (isConnected) {
      requestNodeData();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isConnected]);

  useEffect(() => {
    if (isSignSuccess && walletSignatureData) {
      //send request to node
      console.log('signature', signData);
      console.log('address', address);
      login();
    }
  }, [address, isSignSuccess, login, signData, walletSignatureData]);

  if (!ready) {
    return <Loading />;
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        padding: '0.5rem',
        maxWidth: '400px',
      }}
    >
      <div
        style={{
          marginTop: '1.5rem',
          display: 'grid',
          color: 'white',
          fontSize: '1.25rem',
          fontWeight: '500',
          textAlign: 'center',
        }}
      >
        <span
          style={{
            marginBottom: '0.5rem',
            color: metamaskTitleColor ?? '#fff',
          }}
        >
          Login with Metamask
        </span>
        <div
          style={{
            display: 'flex',
            justifyContent: 'start',
            alignItems: 'center',
            fontSize: '14px',
            color: '#778899',
            whiteSpace: 'break-spaces',
          }}
        >
          <span>
            Choose which account from your wallet you want to log in with
          </span>
        </div>
        <header
          style={{
            marginTop: '1.5rem',
            display: 'flex',
            flexDirection: 'column',
          }}
        >
          <MetaMaskButton
            theme="dark"
            color={isConnected && walletSignatureData ? 'blue' : 'white'}
            buttonStyle={
              isConnected && walletSignatureData
                ? {
                    display: 'flex',
                    justifyContent: 'center',
                    alignItems: 'center',
                    backgroundColor: '#25282D',
                    height: '73px',
                    borderRadius: '6px',
                    border: 'none',
                    outline: 'none',
                  }
                : {
                    cursor: 'pointer',
                  }
            }
          ></MetaMaskButton>
          {isConnected && walletSignatureData && (
            <div style={{ marginTop: '155px' }}>
              <button
                style={{
                  backgroundColor: '#FF7A00',
                  color: 'white',
                  width: '100%',
                  display: 'flex',
                  justifyContent: 'center',
                  alignItems: 'center',
                  gap: '0.5rem',
                  height: '46px',
                  cursor: 'pointer',
                  fontSize: '1rem',
                  fontWeight: '500',
                  borderRadius: '0.375rem',
                  border: 'none',
                  outline: 'none',
                  paddingLeft: '0.5rem',
                  paddingRight: '0.5rem',
                }}
                disabled={isSignLoading}
                onClick={() => signMessage()}
              >
                Sign authentication transaction
              </button>
              {isSignError && (
                <div
                  style={{
                    color: 'red',
                    fontSize: '14px',
                    fontWeight: '500',
                    marginTop: '0.5rem',
                  }}
                >
                  Error signing message
                </div>
              )}
              <div
                style={{
                  color: 'red',
                  fontSize: '14px',
                  fontWeight: '500',
                  marginTop: '0.5rem',
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
          paddingTop: '1rem',
          fontSize: '14px',
          color: '#fff',
          textAlign: 'center',
          cursor: 'pointer',
        }}
        onClick={navigateBack}
      >
        Back to wallet selector
      </div>
    </div>
  );
}
