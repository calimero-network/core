import { useCallback, useState } from 'react';
import { randomBytes } from 'crypto';
import { getOrCreateKeypair } from '../auth/ed25519';

import { useAccount, useSDK, useSignMessage } from '@metamask/sdk-react-ui';

import apiClient from '../api';
import { ResponseData } from '../api/response';
import { getNetworkType } from '../utils/ethWalletType';
import { setStorageNodeAuthorized } from '../auth/storage';
import {
  EthSignatureMessageMetadata,
  LoginRequest,
  LoginResponse,
  NodeChallenge,
  Payload,
  SignatureMessage,
  SignatureMessageMetadata,
  WalletMetadata,
  WalletSignatureData,
} from '../api/dataSource/NodeDataSource';
import { useNavigate } from 'react-router-dom';
import translation from '../constants/en.global.json';
import { useServerDown } from '../context/ServerDownContext';

interface LoginProps {
  setErrorMessage: (msg: string) => void;
  isLogin: boolean;
}

interface RequestNodeDataProps {
  setErrorMessage: (msg: string) => void;
}

interface useMetamaskReturn {
  ready: boolean;
  isConnected: boolean;
  address: `0x${string}` | undefined;
  signData: `0x${string}` | undefined;
  walletSignatureData: WalletSignatureData | null;
  isSignLoading: boolean;
  isSignError: boolean;
  isSignSuccess: boolean;
  signMessage: () => void;
  requestNodeData: ({ setErrorMessage }: RequestNodeDataProps) => Promise<void>;
  login: ({ isLogin, setErrorMessage }: LoginProps) => Promise<void>;
}

const t = translation.useMetamask;

export function useMetamask(): useMetamaskReturn {
  const navigate = useNavigate();
  const { showServerDownPopup } = useServerDown();
  const { chainId, ready } = useSDK();
  const { isConnected, address } = useAccount();
  const [walletSignatureData, setWalletSignatureData] =
    useState<WalletSignatureData | null>(null);

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
  } = useSignMessage({ message: signatureMessage() ?? '' });

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
    [],
  );

  const login = useCallback(
    async ({ isLogin, setErrorMessage }: LoginProps) => {
      setErrorMessage('');
      if (!signData) {
        console.error(t.noSignatureError);
        setErrorMessage(t.noSignatureError);
      } else if (!address) {
        console.error(t.noAddressError);
        setErrorMessage(t.noAddressError);
      } else {
        const walletMetadata: WalletMetadata = {
          wallet: getNetworkType(chainId ?? ''),
          verifyingKey: address,
        };
        if (walletSignatureData?.payload) {
          const metamaskRequest: LoginRequest = {
            walletSignature: signData,
            payload: walletSignatureData.payload,
            walletMetadata: walletMetadata,
          };

          const result: ResponseData<LoginResponse> = isLogin
            ? await apiClient(showServerDownPopup).node().login(metamaskRequest)
            : await apiClient(showServerDownPopup)
                .node()
                .addRootKey(metamaskRequest);

          if (result.error) {
            const errorMessage = isLogin ? t.loginError : t.rootkeyError;
            console.error(errorMessage, result.error);
            setErrorMessage(`${errorMessage}: ${result.error.message}`);
          } else {
            setStorageNodeAuthorized();
            navigate('/identity');
          }
        }
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [address, chainId, signData, walletSignatureData?.payload],
  );

  return {
    ready,
    isConnected,
    address,
    walletSignatureData,
    isSignSuccess,
    isSignLoading,
    signMessage,
    isSignError,
    requestNodeData,
    login,
    signData,
  };
}
