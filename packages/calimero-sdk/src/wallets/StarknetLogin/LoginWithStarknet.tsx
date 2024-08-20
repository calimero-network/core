import React, { useCallback, useEffect, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../../auth/ed25519';

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
import { setStorageNodeAuthorized } from '../../storage/storage';
import { Loading } from '../loading/Loading';
import { getWalletType } from '../eth/type';
import { getStarknet, StarknetWindowObject } from 'get-starknet-core';

interface LoginWithStarknetProps {
  contextId?: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  navigateBack?: () => void | undefined;
}

export function LoginWithStarknet({
  contextId,
  rpcBaseUrl,
  successRedirect,
  navigateBack,
}: LoginWithStarknetProps) {
  const [starknetAccount, setStarknetAccount] = useState<StarknetWindowObject | undefined>();
  const [walletSignatureData, setWalletSignatureData] = useState<WalletSignatureData | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [signData, setSignData] = useState<any>(null);
  const [loading, setLoading] = useState<boolean>(false);

  const walletLogin = useCallback(async (walletType: string) => {
    try {
      setLoading(true);
      const starknetInstance = getStarknet();
      if (starknetInstance) {
        if(walletType === 'argentX') {
          await starknetInstance.enable(window.starknet_argentX);
          const wallets = await starknetInstance.getAvailableWallets();
          const argentX = wallets.find((wallet: any) => wallet.id === 'argentX');
          setStarknetAccount(argentX);
        }else {
          await starknetInstance.enable(window.starknet_metamask);
          const wallets = await starknetInstance.getAvailableWallets();
          const metamask = wallets.find((wallet: any) => wallet.id === 'metamask');
          setStarknetAccount(metamask);
        }
      }
    }catch(error) {
      console.error('Error while login with starknet:', error);
      setErrorMessage('Error while login with starknet');
    }
    
    setLoading(false);
  }, []);

  const requestNodeData = useCallback(async () => {
    try {
      setErrorMessage(null);
      const challengeResponseData: ResponseData<NodeChallenge> = await apiClient
      .node()
      .requestChallenge(rpcBaseUrl, contextId);
      const { publicKey } = await getOrCreateKeypair();

      if (challengeResponseData.error) {
        console.error('requestNodeData error', challengeResponseData.error);
        setErrorMessage("Error while getting node challenge");
        return;
      }

      const signatureMessage: SignatureMessage = {
        nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
        publicKey: publicKey,
      };

      const signatureMessageMetadata: SignatureMessageMetadata = {
        nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
        publicKey: publicKey,
        nonce: challengeResponseData.data?.nonce ?? randomBytes(32).toString('hex'),
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
    }catch(error) {
      console.error('Error requesting node data:', error);
      setErrorMessage('Error requesting node data');
    }
  }, [contextId, rpcBaseUrl]);

  const signMessage = useCallback(async () => {
    try {
      setErrorMessage(null);
      setLoading(true);
      if(starknetAccount) {
        const message = {
          domain: {
            name: "ServerChallenge",
            chainId: "SN_MAIN",
            version: "1",
            revision: "1"
          },
          types: {
            StarknetDomain: [
              { name: "name", type: "shortstring" },
              { name: "chainId", type: "shortstring" },
              { name: "version", type: "shortstring" },
              { name: "revision", type: "shortstring" },
            ],
            Challenge: [
              { name: "nodeSignature", type: "string"},
              { name: "publicKey", type: "string"},
            ],
          },
          primaryType: "Challenge",
          message: {
            nodeSignature: walletSignatureData.payload.message.nodeSignature,
            publicKey: walletSignatureData.payload.message.publicKey
          }
        };
        const signature = await starknetAccount.account.signMessage(message);
        const messageHash = await starknetAccount.account.hashMessage(message);
        if (signature) {
          setSignData({
            signature: signature,
            messageHash: messageHash,
          });
        }
      }
    }catch(error) {
      console.error('Error signing message:', error);
      setErrorMessage('Error signing message');
    }
    setLoading(false);
  }, [starknetAccount, walletSignatureData]);

  const login = useCallback(async () => {
    try {
      setErrorMessage(null);
      setLoading(true);
      if (!signData) {
        console.error('signature is empty');
        setErrorMessage('Signature is empty');
      } else if (!starknetAccount) {
        console.error('address is empty');
        setErrorMessage('Address is empty');
      } else {
        const walletMetadata: WalletMetadata = {
          wallet: getWalletType(starknetAccount?.id),
          signingKey: starknetAccount?.id === 'argentX' ? starknetAccount?.account.address : await starknetAccount?.account.signer.getPubKey(),
          walletAddress: starknetAccount.account.address
        };
        const loginRequest: LoginRequest = {
          walletSignature: signData,
          payload: walletSignatureData?.payload,
          walletMetadata: walletMetadata,
          contextId,
        };
        await apiClient
          .node()
          .login(loginRequest, rpcBaseUrl)
          .then((result) => {
            if (result.error) {
              console.error('Login error: ', result.error);
              setErrorMessage(result.error.message);
            } else {
              setStorageNodeAuthorized();
              successRedirect();
            }
          })
          .catch(() => {
            console.error('error while login!');
            setErrorMessage('Error while login!');
          });
      }
    }catch(error) {
      console.error('Error login:', error);
      setErrorMessage('Error login');
    }
    setLoading(false);
  }, [    
    rpcBaseUrl,
    signData,
    successRedirect,
    walletSignatureData?.payload,
    starknetAccount,
    contextId,
  ]);

  useEffect(() => {
    if (starknetAccount) {
      requestNodeData();
    }
  }, [starknetAccount, requestNodeData]);

  useEffect(() => {
    if (signData && walletSignatureData) {
      login();
    }
  }, [login, signData, walletSignatureData]);

  const logout = useCallback(() => {
    setStarknetAccount(undefined);
    setWalletSignatureData(null);
    setSignData(null);
    setErrorMessage(null);
  }, []);


  if (loading) {
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
            color: '#fff',
          }}
        >
          Login with Starknet
        </span>
        <div
          style={{
            display: 'flex',
            justifyContent: 'center',
            alignItems: 'center',
            fontSize: '14px',
            color: '#778899',
            whiteSpace: 'break-spaces',
          }}
        >
          <span>Choose which account from your wallet you want to log in with</span>
        </div>
        {!starknetAccount && (
          <header
            style={{
              marginTop: '1.5rem',
              display: 'flex',
              minWidth: '500px',
              justifyContent: 'space-between',
              alignItems: 'center',
            }}
          >
          <span
            style={{
              marginTop: '1.5rem',
              display: 'grid',
              fontSize: '1.25rem',
              fontWeight: '500',
              textAlign: 'center',
              marginBottom: '0.5rem',
              color: '#000',
              backgroundColor: '#FFF',
              padding: '0.5rem 0.7rem',
              borderRadius: '0.375rem',
              cursor: 'pointer',
            }}
            onClick={() => walletLogin('argentX')}
          >
            Login with ArgentX
          </span>
          <span
            style={{
              marginTop: '1.5rem',
              display: 'grid',
              fontSize: '1.25rem',
              fontWeight: '500',
              textAlign: 'center',
              marginBottom: '0.5rem',
              color: '#000',
              backgroundColor: '#FFF',
              padding: '0.5rem',
              borderRadius: '0.375rem',
              cursor: 'pointer',
            }}

            onClick={() => walletLogin('metamask')}
          >
            Login with Metamask snap
          </span>
          </header>
        )}
        {starknetAccount && walletSignatureData && (
          <>
            <div style={{ marginTop: '20px' }}>
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
                disabled={starknetAccount === undefined}
                onClick={() => signMessage()}
              >
                Sign authentication transaction
              </button>
            </div>
             <div
             style={{
               paddingTop: '1rem',
               fontSize: '14px',
               color: '#fff',
               textAlign: 'center',
               cursor: 'pointer',
             }}
             onClick={() => logout()}
           >
             Back to Starknet wallet selector
           </div>
          </>
        )}
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
      {errorMessage && (
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
      )}
    </div>
  );
}
