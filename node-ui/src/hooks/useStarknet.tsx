import { useCallback, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../auth/ed25519';

import apiClient from '../api';
import {
  EthSignatureMessageMetadata,
  LoginRequest,
  LoginResponse,
  NodeChallenge,
  Payload,
  SignatureMessage,
  SignatureMessageMetadata,
  SignData,
  WalletMetadata,
  WalletSignatureData,
} from '../api/dataSource/NodeDataSource';
import { ResponseData } from '../api/response';
import { useNavigate } from 'react-router-dom';
import { setStorageNodeAuthorized } from '../auth/storage';
import { useServerDown } from '../context/ServerDownContext';
import { getWalletType } from '../utils/starknetWalletType';
import { getStarknet, StarknetWindowObject } from 'get-starknet-core';
import { constants, Signature } from 'starknet';
import translations from '../constants/en.global.json';

interface LoginProps {
  setErrorMessage: (msg: string) => void;
  isLogin: boolean;
}

interface RequestNodeDataProps {
  setErrorMessage: (msg: string) => void;
}

interface useStarknetReturn {
  login: ({ isLogin, setErrorMessage }: LoginProps) => void;
  walletLogin: (walletType: string, setErrorMessage: (msg: string) => void) => void;
  ready: boolean;
  starknetInstance: StarknetWindowObject | null;
  argentXId: string;
  walletSignatureData: WalletSignatureData | null;
  signData: SignData | null;
  signMessage: (setErrorMessage: (msg: string) => void) => void;
  logout: (setErrorMessage: (msg: string) => void) => void;
  requestNodeData: ({ setErrorMessage }: RequestNodeDataProps) => void;
  changeMetamaskNetwork: (networkId: string, setErrorMessage: (msg: string) => void) => void;
}

const t = translations.useStarknet;

export function useStarknet(): useStarknetReturn  {
  const [starknetInstance, setStarknetInstance] =
    useState<StarknetWindowObject | null>(null);
  const [walletSignatureData, setWalletSignatureData] =
    useState<WalletSignatureData | null>(null);
  const [signData, setSignData] = useState<SignData | null>(null);
  const [ready, setReady] = useState<boolean>(true);
  const { showServerDownPopup } = useServerDown();
  const navigate = useNavigate();

  const argentXId = 'argentX';

  const walletLogin = useCallback(async (walletType: string, setErrorMessage: (msg: string) => void) => {
    try {
      setReady(false);
      const starknetInstance = getStarknet();
      if (starknetInstance) {
        if (walletType === argentXId) {
          if(window.starknet_argentX) {
            await starknetInstance.enable(window.starknet_argentX);
            const wallets: StarknetWindowObject[] =
            await starknetInstance.getAvailableWallets();
            const argentX: StarknetWindowObject = wallets.find(
              (wallet: any) => wallet.id === argentXId,
            ) as StarknetWindowObject;
            setStarknetInstance(argentX);
          }else {
            setErrorMessage(t.walletNotFound);
          }
        } else {
          if(window.starknet_metamask) {
            await starknetInstance.enable(window.starknet_metamask);
            const wallets: StarknetWindowObject[] =
              await starknetInstance.getAvailableWallets();
            const metamask: StarknetWindowObject = wallets.find(
              (wallet: any) => wallet.id === 'metamask',
            ) as StarknetWindowObject;
            setStarknetInstance(metamask);
          }else {
            setErrorMessage(t.walletNotFound);
          }
        }
      }
    } catch (error) {
      console.error(`${t.errorLogin}: ${error}`);
      setErrorMessage(t.errorLogin);
    }

    setReady(true);
  }, []);

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
    },
    [showServerDownPopup],
  );

  const changeMetamaskNetwork = useCallback(async (networkId: string, setErrorMessage: (msg: string) => void) => {
    try {
      setReady(false);
      setErrorMessage('');
      if (starknetInstance) {
        await starknetInstance.request({
          type: "wallet_switchStarknetChain",
          params: {
            id: starknetInstance.id,
            chainId: networkId === constants.NetworkName.SN_MAIN ? constants.StarknetChainId.SN_MAIN : constants.StarknetChainId.SN_SEPOLIA,
            baseUrl: networkId === constants.NetworkName.SN_MAIN ? constants.BaseUrl.SN_MAIN : constants.BaseUrl.SN_SEPOLIA,
            chainName: networkId === constants.NetworkName.SN_MAIN ? constants.NetworkName.SN_MAIN : constants.NetworkName.SN_SEPOLIA,
          }
        })
      }
    } catch (error) {
      console.error(`${t.errorChangingNetwork}: ${error}`);
      setErrorMessage(`${t.errorChangingNetwork}`);
    }
    setReady(true);
  }, [starknetInstance]);

  const signMessage = useCallback(async (setErrorMessage: (msg: string) => void) => {
    try {
      setErrorMessage('');
      setReady(false);
      let chainId: string = starknetInstance?.chainId === 'SN_MAIN' ? constants.StarknetChainId.SN_MAIN : constants.StarknetChainId.SN_SEPOLIA;
      if(starknetInstance && starknetInstance.id !== argentXId) {
        chainId = starknetInstance.chainId === constants.StarknetChainId.SN_MAIN ? constants.StarknetChainId.SN_MAIN : constants.StarknetChainId.SN_SEPOLIA;
      }
      if (starknetInstance && walletSignatureData) {
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
            nodeSignature: walletSignatureData.payload?.message.nodeSignature,
            publicKey: walletSignatureData.payload?.message.publicKey,
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
      console.error(`${t.signMessageError}: ${error}`);
      setErrorMessage(t.signMessageError);
    }
    setReady(true);
  }, [starknetInstance, walletSignatureData]);

  const login = useCallback(
    async ({ isLogin, setErrorMessage }: LoginProps) => {
      setReady(false);
      setErrorMessage('');
      if (!signData) {
        console.error(t.noSignatureError);
        setErrorMessage(t.noSignatureError);
      } else if (!starknetInstance) {
        console.error(t.noAddressError);
        setErrorMessage(t.noAddressError);
      } else if(starknetInstance && walletSignatureData && walletSignatureData.payload) {
        let chainId: string = starknetInstance.chainId === 'SN_MAIN' ? constants.StarknetChainId.SN_MAIN : constants.StarknetChainId.SN_SEPOLIA;
        let rpcUrl: string = starknetInstance.chainId === 'SN_MAIN' ? constants.RPC_NODES.SN_MAIN[0] : constants.RPC_NODES.SN_SEPOLIA[0];
        if(starknetInstance.id !== argentXId) {
          chainId = starknetInstance.chainId === constants.StarknetChainId.SN_MAIN ? constants.StarknetChainId.SN_MAIN : constants.StarknetChainId.SN_SEPOLIA;
          rpcUrl = starknetInstance.chainId === constants.StarknetChainId.SN_MAIN ? constants.RPC_NODES.SN_MAIN[0] : constants.RPC_NODES.SN_SEPOLIA[0];
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
        const starknetLoginRequest: LoginRequest = {
          walletSignature: signData,
          payload: walletSignatureData?.payload,
          walletMetadata: walletMetadata,
        };
        const result: ResponseData<LoginResponse> = isLogin
          ? await apiClient(showServerDownPopup).node().login(starknetLoginRequest)
          : await apiClient(showServerDownPopup)
              .node()
              .addRootKey(starknetLoginRequest);

        if (result.error) {
          const errorMessage = isLogin ? t.loginError : t.rootkeyError;
          console.error(errorMessage, result.error);
          setErrorMessage(`${errorMessage}: ${result.error.message}`);
        } else {
          setStorageNodeAuthorized();
          navigate('/identity');
        }
      }
      setReady(true);
    }, [navigate, signData, starknetInstance, showServerDownPopup, walletSignatureData],
  );

  const logout = useCallback((setErrorMessage: (msg: string) => void) => {
    setStarknetInstance(null);
    setWalletSignatureData(null);
    setSignData(null);
    setErrorMessage('');
  }, []);

  return {
    ready,
    walletLogin,
    changeMetamaskNetwork,
    login,
    starknetInstance,
    argentXId,
    walletSignatureData,
    signData,
    signMessage,
    logout,
    requestNodeData,
  };
}
