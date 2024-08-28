import React, { useCallback, useEffect, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../../auth/ed25519';

import apiClient from '../../api';
import {
  EthSignatureMessageMetadata,
  NodeChallenge,
  Payload,
  RootKeyRequest,
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
import { constants, Signature } from 'starknet';

interface StarknetRootKeyProps {
  contextId?: string;
  rpcBaseUrl: string;
  successRedirect: () => void;
  navigateBack?: () => void | undefined;
}

export function StarknetRootKey({
  contextId,
  rpcBaseUrl,
  successRedirect,
  navigateBack,
}: StarknetRootKeyProps) {
  const [starknetInstance, setStarknetInstance] =
    useState<StarknetWindowObject | null>(null);
  const [walletSignatureData, setWalletSignatureData] =
    useState<WalletSignatureData | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [signData, setSignData] = useState<any>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const argentXId = 'argentX';

  const walletLogin = useCallback(async (walletType: string) => {
    try {
      setLoading(true);
      const starknet = getStarknet();
      if (starknet) {
        if (walletType === argentXId) {
          await starknet.enable(window.starknet_argentX);
          const wallets: StarknetWindowObject[] =
            await starknet.getAvailableWallets();
          const argentX: StarknetWindowObject = wallets.find(
            (wallet: any) => wallet.id === argentXId,
          );
          setStarknetInstance(argentX);
        } else {
          await starknet.enable(window.starknet_metamask);
          const wallets: StarknetWindowObject[] =
            await starknet.getAvailableWallets();
          const metamask: StarknetWindowObject = wallets.find(
            (wallet: any) => wallet.id === 'metamask',
          );
          setStarknetInstance(metamask);
        }
      }
      setLoading(false);
    } catch (error) {
      console.error('Error logging in:', error);
      setErrorMessage('Error logging in');
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
        setErrorMessage('Error while getting node challenge');
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
        timestamp:
          challengeResponseData.data?.timestamp ?? new Date().getTime(),
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
    } catch (error) {
      console.error('Error requesting node data:', error);
      setErrorMessage('Error requesting node data');
    }
  }, [contextId, rpcBaseUrl]);

  const changeMetamaskNetwork = useCallback(
    async (networkId: string) => {
      try {
        setLoading(true);
        setErrorMessage(null);
        await starknetInstance.request({
          type: 'wallet_switchStarknetChain',
          params: {
            id: starknetInstance.id,
            chainId:
              networkId === constants.NetworkName.SN_MAIN
                ? constants.StarknetChainId.SN_MAIN
                : constants.StarknetChainId.SN_SEPOLIA,
            baseUrl:
              networkId === constants.NetworkName.SN_MAIN
                ? constants.BaseUrl.SN_MAIN
                : constants.BaseUrl.SN_SEPOLIA,
            chainName:
              networkId === constants.NetworkName.SN_MAIN
                ? constants.NetworkName.SN_MAIN
                : constants.NetworkName.SN_SEPOLIA,
          },
        });
      } catch (error) {
        console.error('Error changing network:', error);
        setErrorMessage('Error changing network');
      }
      setLoading(false);
    },
    [starknetInstance],
  );

  const signMessage = useCallback(async () => {
    try {
      setErrorMessage(null);
      setLoading(true);
      if (starknetInstance) {
        let chainId: string =
          starknetInstance.chainId === 'SN_MAIN'
            ? constants.StarknetChainId.SN_MAIN
            : constants.StarknetChainId.SN_SEPOLIA;
        if (starknetInstance.id !== argentXId) {
          chainId =
            starknetInstance.chainId === constants.StarknetChainId.SN_MAIN
              ? constants.StarknetChainId.SN_MAIN
              : constants.StarknetChainId.SN_SEPOLIA;
        }
        const message = {
          domain: {
            name: 'ServerChallenge',
            chainId: chainId,
            version: '1',
            revision: '1',
          },
          types: {
            StarknetDomain: [
              { name: 'name', type: 'shortstring' },
              { name: 'chainId', type: 'felt' },
              { name: 'version', type: 'shortstring' },
              { name: 'revision', type: 'shortstring' },
            ],
            Challenge: [
              { name: 'nodeSignature', type: 'string' },
              { name: 'publicKey', type: 'string' },
            ],
          },
          primaryType: 'Challenge',
          message: {
            nodeSignature: walletSignatureData.payload.message.nodeSignature,
            publicKey: walletSignatureData.payload.message.publicKey,
          },
        };
        const signature: Signature =
          await starknetInstance.account.signMessage(message);
        const messageHash: String =
          await starknetInstance.account.hashMessage(message);

        if (signature) {
          setSignData({
            signature: signature,
            messageHash: messageHash,
          });
        }
      }
    } catch (error) {
      console.error('Error signing message:', error);
      setErrorMessage('Error signing message');
    }
    setLoading(false);
  }, [starknetInstance, walletSignatureData]);

  const addRootKey = useCallback(async () => {
    try {
      setErrorMessage(null);
      setLoading(true);
      if (!signData) {
        console.error('signature is empty');
        setErrorMessage('Signature is empty');
      } else if (!starknetInstance) {
        console.error('address is empty');
        setErrorMessage('Address is empty');
      } else {
        let chainId: string =
          starknetInstance.chainId === 'SN_MAIN'
            ? constants.StarknetChainId.SN_MAIN
            : constants.StarknetChainId.SN_SEPOLIA;
        let rpcUrl: string =
          starknetInstance.chainId === 'SN_MAIN'
            ? constants.RPC_NODES.SN_MAIN[0]
            : constants.RPC_NODES.SN_SEPOLIA[0];
        if (starknetInstance.id !== argentXId) {
          chainId =
            starknetInstance.chainId === constants.StarknetChainId.SN_MAIN
              ? constants.StarknetChainId.SN_MAIN
              : constants.StarknetChainId.SN_SEPOLIA;
          rpcUrl =
            starknetInstance.chainId === constants.StarknetChainId.SN_MAIN
              ? constants.RPC_NODES.SN_MAIN[0]
              : constants.RPC_NODES.SN_SEPOLIA[0];
        }
        const walletMetadata: WalletMetadata = {
          wallet: getWalletType(starknetInstance?.id),
          verifyingKey:
            starknetInstance?.id === argentXId
              ? starknetInstance?.account.address
              : await starknetInstance?.account.signer.getPubKey(),
          walletAddress: starknetInstance.account.address,
          networkMetadata: {
            chainId: chainId,
            rpcUrl: rpcUrl,
          },
        };
        const rootKeyRequest: RootKeyRequest = {
          walletSignature: signData,
          payload: walletSignatureData?.payload,
          walletMetadata: walletMetadata,
          contextId,
        };
        await apiClient
          .node()
          .addRootKey(rootKeyRequest, rpcBaseUrl, contextId)
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
    } catch (error) {
      console.error('Error adding root key:', error);
      setErrorMessage('Error adding root key');
    }
    setLoading(false);
  }, [
    rpcBaseUrl,
    signData,
    successRedirect,
    walletSignatureData?.payload,
    starknetInstance,
    contextId,
  ]);

  useEffect(() => {
    if (starknetInstance) {
      requestNodeData();
    }
  }, [starknetInstance, requestNodeData]);

  useEffect(() => {
    if (signData && walletSignatureData) {
      //send request to node
      addRootKey();
    }
  }, [addRootKey, signData, walletSignatureData]);

  const logout = useCallback(() => {
    setStarknetInstance(null);
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
          Add root key with Starknet
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
          <span>
            Choose which account from your wallet you want to add root key with
          </span>
        </div>
        {!starknetInstance && (
          <header
            style={{
              marginTop: '1.5rem',
              display: 'flex',
              minWidth: '500px',
              justifyContent: 'space-between',
              alignItems: 'center',
              flexDirection: 'column',
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
              Add root key with ArgentX
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
              Add root key with Metamask snap
            </span>
          </header>
        )}
        {starknetInstance && walletSignatureData && (
          <>
            {starknetInstance?.id !== argentXId && (
              <div
                style={{
                  marginTop: '1.5rem',
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: 'center',
                }}
              >
                <label htmlFor="network" style={{ marginRight: 'auto' }}>
                  Current network:
                </label>
                <select
                  name="network"
                  style={{ width: '100%', height: '46px' }}
                  defaultValue={
                    starknetInstance.chainId ===
                    constants.StarknetChainId.SN_MAIN
                      ? constants.NetworkName.SN_MAIN
                      : constants.NetworkName.SN_SEPOLIA
                  }
                  onChange={(e) => changeMetamaskNetwork(e.target.value)}
                >
                  <option value={constants.NetworkName.SN_MAIN}>Mainnet</option>
                  <option value={constants.NetworkName.SN_SEPOLIA}>
                    Sepolia
                  </option>
                </select>
              </div>
            )}
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
                disabled={starknetInstance === null}
                onClick={() => signMessage()}
              >
                Sign root key transaction
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
