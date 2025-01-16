import { useCallback, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../auth/ed25519';

import { DelegationIdentity } from '@dfinity/identity';

import apiClient from '../api';
import {
  IcpSignatureMessageMetadata,
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
import { getAppEndpointKey } from '../utils/storage';

interface RequestNodeDataProps {
  setErrorMessage: (msg: string) => void;
}

/**
 * The `useIcpReturn` interface encapsulates the state and methods needed to interact with the Internet Computer (IC) for user authentication, network switching, and signing messages.
 *
 * This interface provides a hook-based approach to managing the user's authentication status, including requesting node data, signing messages for login or root key addition, and handling network changes.
 * It manages the underlying state for wallet signatures, user readiness, and interactions with the IC Identity service.
 *
 * Key functionalities include:
 * - Requesting node challenge data for signing.
 * - Logging in or adding a root key by signing a challenge.
 * - Switching between different Internet Computer networks (production or staging).
 * - Managing the user's authentication state and error handling throughout the process.
 */
interface useIcpReturn {
  /**
   * Indicates whether the system is ready for a new action (e.g., switching networks or signing messages).
   * This is useful for showing loading indicators while operations are in progress.
   */
  ready: boolean;

  /**
   * Holds the data related to the wallet signature after the user has interacted with the Internet Computer (IC).
   * This includes the signature payload, the public key, and other metadata required to authenticate or sign transactions.
   * Initially, it's null, and is populated after requesting node data.
   */
  walletSignatureData: WalletSignatureData | null;

  /**
   * Holds the sign data object which might include any cryptographic information needed for the signing process.
   * Initially, it's null and set once the signing process is completed.
   */
  signData: SignData | null;

  /**
   * Signs a message and handles the login or adding a root key based on the `isLogin` parameter.
   * This function orchestrates the authentication flow, signing the challenge with the Internet Computer Identity, and logs in the user.
   * @param isLogin - Boolean flag to indicate if this is a login (`true`) or a root key addition (`false`).
   * @param setErrorMessage - Function to set any error messages that occur during the process.
   */
  signMessageAndLogin: (
    isLogin: boolean,
    setErrorMessage: (msg: string) => void,
  ) => void;

  /**
   * Logs out the user by clearing the wallet signature data and sign data.
   * Resets any error messages and ensures the user is logged out from the current session.
   * @param setErrorMessage - Function to clear or set any error messages during the logout process.
   */
  logout: (setErrorMessage: (msg: string) => void) => void;

  /**
   * Requests data from the node to start the signing process, such as the challenge.
   * It retrieves the node's challenge and prepares a signature payload using the wallet's public key.
   * @param setErrorMessage - Function to set any error messages if the request fails.
   */
  requestNodeData: ({ setErrorMessage }: RequestNodeDataProps) => void;

  /**
   * Changes the network the user is interacting with (e.g., from production to staging).
   * Updates the current network and resets the state as necessary.
   * @param networkId - The network ID (key of possibleNetworks) to switch to.
   * @param setErrorMessage - Function to set any error messages if changing the network fails.
   */
  changeNetwork: (
    networkId: NetworkId,
    setErrorMessage: (msg: string) => void,
  ) => void;
}

const t = translations.useIcp;

/**
 * A dictionary of possible Internet Computer (IC) networks that the user can interact with.
 * Each network has a URL and a corresponding canister ID for authentication.
 */
const possibleNetworks = {
  production: {
    url: 'https://identity.ic0.app',
    canisterId: 'rdmx6-jaaaa-aaaaa-aaadq-cai',
  },
  staging: {
    url: 'https://beta.identity.ic0.app/',
    canisterId: 'fgte5-ciaaa-aaaad-aaatq-cai',
  },
};

export type NetworkId = keyof typeof possibleNetworks;

export function useIcp(): useIcpReturn {
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
      const signatureMetadata: IcpSignatureMessageMetadata = {};
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
      if (!getAppEndpointKey()) {
        navigate('/');
        return;
      }
      try {
        setErrorMessage('');
        setReady(false);
        if (walletSignatureData) {
          if (walletSignatureData.payload?.message.message) {
            const message = new Uint8Array(
              Buffer.from(walletSignatureData.payload?.message.message),
            );
            const encodedChallenge = new Uint8Array(Buffer.from(message));
            let delegationIdentity;
            try {
              delegationIdentity = await authWithII({
                url: currentNetwork.url,
                sessionPublicKey: encodedChallenge,
              });
            } catch (error: any) {
              console.error('Error:', error);
              setErrorMessage(
                error.message || 'An error occurred during authentication.',
              );
              setReady(true);
              return;
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
                  return Array.from(v, (byte) =>
                    byte.toString(16).padStart(2, '0'),
                  ).join('');
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
                wallet: WalletType.ICP({
                  canisterId: currentNetwork.canisterId,
                  walletName: 'Internet Identity',
                }),
                verifyingKey: publicKey,
              };
              if (walletSignatureData?.payload) {
                const IcpRequest: LoginRequest = {
                  walletSignature: JSON.stringify(delegationChain),
                  payload: walletSignatureData.payload,
                  walletMetadata: walletMetadata,
                };

                const result: ResponseData<LoginResponse> = isLogin
                  ? await apiClient(showServerDownPopup)
                      .node()
                      .login(IcpRequest)
                  : await apiClient(showServerDownPopup)
                      .node()
                      .addRootKey(IcpRequest);

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
    [walletSignatureData, currentNetwork, navigate, showServerDownPopup],
  );

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
        }, 500);
      });

      // Wait for II to say it's ready
      const readyPromise = new Promise<MessageEvent>((resolve) => {
        const readyHandler = (e: MessageEvent) => {
          // Only process messages from II
          if (e.origin !== iiUrl.origin) {
            return; // Ignore messages from other origins
          }

          if (e.data?.kind !== 'authorize-ready') {
            return; // Ignore messages with wrong kind
          }

          window.removeEventListener('message', readyHandler);
          resolve(e);
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
      const message = res.data;
      return message;
    } catch (error) {
      console.error('Error:', error);
      throw error;
    }
  };

  const logout = useCallback((setErrorMessage: (msg: string) => void) => {
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
