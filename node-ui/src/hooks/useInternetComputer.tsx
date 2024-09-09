import { useCallback, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../auth/ed25519';

import { DelegationIdentity } from '@dfinity/identity';

import apiClient from '../api';
import {
  InternetComputerSignatureMessageMetadata,
  NodeChallenge,
  Payload,
  SignatureMessage,
  SignatureMessageMetadata,
  SignData,
  WalletSignatureData,
  LoginRequest,
  LoginResponse,
  WalletMetadata,
  WalletType,
} from '../api/dataSource/NodeDataSource';
import { ResponseData } from '../api/response';
import { useNavigate } from 'react-router-dom';
import { setStorageNodeAuthorized } from '../auth/storage';
import { useServerDown } from '../context/ServerDownContext';
import translations from '../constants/en.global.json';

interface RequestNodeDataProps {
  setErrorMessage: (msg: string) => void;
}

interface useInternetComputerReturn {
  ready: boolean;
  walletSignatureData: WalletSignatureData | null;
  signData: SignData | null;
  signMessageAndLogin: (
    isLogin: boolean,
    setErrorMessage: (msg: string) => void,
  ) => void;
  logout: (setErrorMessage: (msg: string) => void) => void;
  requestNodeData: ({ setErrorMessage }: RequestNodeDataProps) => void;
  changeNetwork: (
    networkId: NetworkId,
    setErrorMessage: (msg: string) => void,
  ) => void;
}

const t = translations.useInternetComputer;
const possibleNetworks = {
  production: {
    url: 'https://identity.ic0.app',
    cannisterId: 'rdmx6-jaaaa-aaaaa-aaadq-cai',
  },
  staging: {
    url: 'https://beta.identity.ic0.app/',
    cannisterId: 'fgte5-ciaaa-aaaad-aaatq-cai',
  },
};

export type NetworkId = keyof typeof possibleNetworks;

export function useInternetComputer(): useInternetComputerReturn {
  const navigate = useNavigate();
  const [ready, setReady] = useState<boolean>(true);
  const { showServerDownPopup } = useServerDown();
  const [walletSignatureData, setWalletSignatureData] =
    useState<WalletSignatureData | null>(null);
  const [signData, setSignData] = useState<SignData | null>(null);

  const [currentNetwork, setCurrentNetwork] = useState(
    possibleNetworks['production'],
  );

  const requestNodeData = useCallback(
    async ({ setErrorMessage }: RequestNodeDataProps) => {
      const challengeResponseData: ResponseData<NodeChallenge> =
        await apiClient(showServerDownPopup).node().requestChallenge();
      const { publicKey } = await getOrCreateKeypair();

      if (challengeResponseData.error) {
        console.error(
          `${t.requestNodeDataError}: ${challengeResponseData.error}`,
        );
        setErrorMessage(
          `${t.requestNodeDataError}: ${challengeResponseData.error}`,
        );
        return;
      }

      const signatureMessage: SignatureMessage = {
        nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
        publicKey: publicKey,
      };

      const signatureMessageMetadata: SignatureMessageMetadata = {
        publicKey: publicKey,
        nodeSignature: challengeResponseData.data?.nodeSignature ?? '',
        nonce:
          challengeResponseData.data?.nonce ?? randomBytes(32).toString('hex'),
        timestamp:
          challengeResponseData.data?.timestamp ?? new Date().getTime(),
        message: JSON.stringify(signatureMessage),
      };
      const signatureMetadata: InternetComputerSignatureMessageMetadata = {};
      const payload: Payload = {
        message: signatureMessageMetadata,
        metadata: signatureMetadata,
      };
      const wsd: WalletSignatureData = {
        payload,
        publicKey,
      };
      setWalletSignatureData(wsd);
    },
    [showServerDownPopup],
  );

  const changeNetwork = useCallback(
    async (networkId: NetworkId, setErrorMessage: (msg: string) => void) => {
      try {
        setReady(false);
        setErrorMessage('');
        console.log('Changing network to:', networkId);
        setCurrentNetwork(possibleNetworks[networkId]);
      } catch (error) {
        console.error(`${t.errorChangingNetwork}: ${error}`);
        setErrorMessage(`${t.errorChangingNetwork}`);
      }
      setReady(true);
    },
    [],
  );

  const signMessageAndLogin = useCallback(
    async (isLogin: boolean, setErrorMessage: (msg: string) => void) => {
      try {
        setErrorMessage('');
        setReady(false);

        console.log('walletSignatureData:', walletSignatureData);

        // console.log('currentNetwork:', currentNetwork);
        // return;
        if (walletSignatureData) {
          if (walletSignatureData.payload?.message.message) {
            const message = new Uint8Array(
              Buffer.from(walletSignatureData.payload?.message.message),
            );
            const encodedChallenge = new Uint8Array(Buffer.from(message));
            console.log('Current Network:', currentNetwork);
            let delegationIdentity;
            try {
              delegationIdentity = await authWithII({
                url: currentNetwork.url,
                sessionPublicKey: encodedChallenge,
              });
              console.log('delegationIdentity:', delegationIdentity);
            } catch (error: any) {
              console.error('Error:', error);
              setErrorMessage(
                error.message || 'An error occurred during authentication.',
              );
              setReady(true);
              return; // Exit the function if there's an error
            }

            if (delegationIdentity) {
              const data = { encodedChallenge, delegationIdentity };
              // Serialize the data with BigInt and Uint8Array handling
              const serializedData = JSON.stringify(data, (_, v) => {
                if (typeof v === 'bigint') {
                  // Convert BigInt to hex string
                  return v.toString(16);
                }
                if (v instanceof Uint8Array) {
                  // Convert Uint8Array to hex string
                  return uint8ArrayToHexString(v);
                }
                return v;
              });
              const jsonData = JSON.parse(serializedData);
              const delegationChain = {
                delegations: jsonData.delegationIdentity.delegations,
                publicKey: jsonData.delegationIdentity.userPublicKey,
              };
              const publicKey: string = Buffer.from(
                jsonData.delegationIdentity.userPublicKey,
              ).toString('base64');

              const walletMetadata: WalletMetadata = {
                wallet: WalletType.INTERNETCOMPUTER({
                  cannisterId: currentNetwork.cannisterId,
                  walletName: 'Internet Identity',
                }),
                verifyingKey: publicKey,
              };
              if (walletSignatureData?.payload) {
                const internetComputerRequest: LoginRequest = {
                  walletSignature: JSON.stringify(delegationChain),
                  payload: walletSignatureData.payload,
                  walletMetadata: walletMetadata,
                };

                console.log(
                  'internetComputerRequest:',
                  internetComputerRequest,
                );

                const result: ResponseData<LoginResponse> = isLogin
                  ? await apiClient(showServerDownPopup)
                      .node()
                      .login(internetComputerRequest)
                  : await apiClient(showServerDownPopup)
                      .node()
                      .addRootKey(internetComputerRequest);

                if (result.error) {
                  const errorMessage = isLogin ? t.loginError : t.rootkeyError;
                  console.error(errorMessage, result.error);
                  setErrorMessage(`${errorMessage}: ${result.error.message}`);
                } else {
                  setStorageNodeAuthorized();
                  navigate('/identity');
                }
              }
            } else {
              console.error(`${t.signMessageError}`);
              setErrorMessage(t.signMessageError);
            }
          }
        }
      } catch (error) {
        console.error(`${t.signMessageError}: ${error}`);
        setErrorMessage(t.signMessageError);
      } finally {
        setReady(true);
      }
    },
    [walletSignatureData, currentNetwork],
  );

  // Helper function to convert Uint8Array to hex string
  function uint8ArrayToHexString(arr: Uint8Array) {
    return Array.from(arr, (byte) => byte.toString(16).padStart(2, '0')).join(
      '',
    );
  }

  const authWithII = async ({
    url: url_,
    maxTimeToLive,
    derivationOrigin,
    sessionPublicKey,
  }: {
    url: string;
    maxTimeToLive?: bigint;
    derivationOrigin?: string;
    sessionPublicKey: Uint8Array;
  }): Promise<DelegationIdentity> => {
    try {
      // Figure out the II URL to use
      const iiUrl = new URL(url_);
      iiUrl.hash = '#authorize';

      // Open an II window and kickstart the flow
      const win = window.open(iiUrl, '_blank', 'width=500,height=700');
      if (win === null) {
        throw new Error(`Could not open window for '${iiUrl}'`);
      }

      // Create a promise that rejects if the window is closed by the user
      const windowClosedPromise = new Promise<never>((_, reject) => {
        const checkWindowClosed = setInterval(() => {
          if (win.closed) {
            clearInterval(checkWindowClosed);
            reject(new Error('User closed the window.'));
          }
        }, 500); // Check every 500ms
      });

      // Wait for II to say it's ready
      const readyPromise = new Promise<MessageEvent>((resolve, reject) => {
        const readyHandler = (e: MessageEvent) => {
          window.removeEventListener('message', readyHandler);
          console.log('Received message:', e);
          if (e.origin !== iiUrl.origin || e.data.kind !== 'authorize-ready') {
            win.close();
            reject(new Error('Bad message from II window. Please try again.'));
          } else {
            resolve(e);
          }
        };
        window.addEventListener('message', readyHandler);
      });

      // Wait for either the window to be closed or II to be ready
      await Promise.race([windowClosedPromise, readyPromise]);

      // Send the request to II
      const request = {
        kind: 'authorize-client',
        sessionPublicKey,
        maxTimeToLive,
        derivationOrigin,
      };

      win.postMessage(request, iiUrl.origin);

      // Wait for the II response and update the local state
      const responsePromise = new Promise<MessageEvent>((resolve, reject) => {
        const responseHandler = (e: MessageEvent) => {
          if (e.origin === iiUrl.origin) {
            window.removeEventListener('message', responseHandler);
            win.close();
            if (e.data.kind !== 'authorize-client-success') {
              reject(new Error('Bad reply: ' + JSON.stringify(e.data)));
            } else {
              resolve(e);
            }
          }
        };
        window.addEventListener('message', responseHandler);
      });

      // Ensure the window monitoring continues until the response is received
      const res = await Promise.race([windowClosedPromise, responsePromise]);
      console.log('Received response:', res);
      const message = res.data;
      return message;
    } catch (error) {
      console.log('Error:', error);
      console.error('Error:', error);
      throw error;
    }
  };

  const logout = useCallback((setErrorMessage: (msg: string) => void) => {
    // setStarknetInstance(null);
    setWalletSignatureData(null);
    setSignData(null);
    setErrorMessage('');
  }, []);

  return {
    ready,
    walletSignatureData,
    signData,
    signMessageAndLogin,
    logout,
    requestNodeData,
    changeNetwork,
  };
}
