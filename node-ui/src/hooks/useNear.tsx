import { useCallback } from 'react';
import type { AccountView } from 'near-api-js/lib/providers/provider';
import { WalletSelector } from '@near-wallet-selector/core';
import { providers } from 'near-api-js';
import {
  verifyFullKeyBelongsToUser,
  verifySignature,
  type SignedMessage,
  type SignMessageParams,
  type AccountState,
} from '@near-wallet-selector/core';
import {
  LoginRequest,
  LoginResponse,
  NearSignatureMessageMetadata,
  Payload,
  SignatureMessageMetadata,
  WalletMetadata,
  WalletSignatureData,
  WalletType,
  NodeChallenge,
  SignatureMessage,
} from '../api/dataSource/NodeDataSource';
import apiClient from '../api';
import { ResponseData } from '../api/response';
import { setStorageNodeAuthorized } from '../auth/storage';
import { useNavigate } from 'react-router-dom';
import { randomBytes } from 'crypto';
import { WalletSelectorModal } from '@near-wallet-selector/modal-ui/src/lib/modal.types';

import { Buffer } from 'buffer';
import * as nearAPI from 'near-api-js';
import { Package, Release } from '../pages/Applications';
import { getOrCreateKeypair } from '../auth/ed25519';
import { Account } from '../components/near/NearWallet';
import translation from '../constants/en.global.json';
import { useServerDown } from '../context/ServerDownContext';
import { getNearEnvironment } from '../utils/node';

const JSON_RPC_ENDPOINT = 'https://rpc.testnet.near.org';
// @ts-ignore

const t = translation.useNear;

export function useRPC() {
  const { showServerDownPopup } = useServerDown();
  const getPackages = async (): Promise<Package[]> => {
    const provider = new nearAPI.providers.JsonRpcProvider({
      url: JSON_RPC_ENDPOINT,
    });

    const rawResult = await provider.query({
      request_type: 'call_function',
      account_id: 'calimero-package-manager.testnet',
      method_name: 'get_packages',
      args_base64: btoa(
        JSON.stringify({
          offset: 0,
          limit: 100,
        }),
      ),
      finality: 'final',
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getPackage = async (id: string): Promise<Package | null> => {
    try {
      const provider = new nearAPI.providers.JsonRpcProvider({
        url: JSON_RPC_ENDPOINT,
      });

      const rawResult = await provider.query({
        request_type: 'call_function',
        account_id: 'calimero-package-manager.testnet',
        method_name: 'get_package',
        args_base64: btoa(
          JSON.stringify({
            id,
          }),
        ),
        finality: 'final',
      });
      // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
      return JSON.parse(Buffer.from(rawResult.result).toString());
    } catch (e) {
      //If there is no package available, there is high possibility that context contains local wasm for development
      console.log('Error getting package', e);
      return null;
    }
  };

  const getReleases = async (id: string): Promise<Release[]> => {
    const provider = new nearAPI.providers.JsonRpcProvider({
      url: JSON_RPC_ENDPOINT,
    });

    const rawResult = await provider.query({
      request_type: 'call_function',
      account_id: 'calimero-package-manager.testnet',
      method_name: 'get_releases',
      args_base64: btoa(
        JSON.stringify({
          id,
          offset: 0,
          limit: 100,
        }),
      ),
      finality: 'final',
    });
    // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
    return JSON.parse(Buffer.from(rawResult.result).toString());
  };

  const getLatestRelease = async (id: string): Promise<Release | null> => {
    const provider = new nearAPI.providers.JsonRpcProvider({
      url: JSON_RPC_ENDPOINT,
    });
    try {
      const rawResult = await provider.query({
        request_type: 'call_function',
        account_id: 'calimero-package-manager.testnet',
        method_name: 'get_releases',
        args_base64: btoa(
          JSON.stringify({
            id,
            offset: 0,
            limit: 100,
          }),
        ),
        finality: 'final',
      });
      // @ts-expect-error: Property 'result' does not exist on type 'QueryResponseKind'
      const releases = JSON.parse(Buffer.from(rawResult.result).toString());
      if (releases.length === 0) {
        return null;
      }
      return releases[releases.length - 1];
    } catch (e) {
      console.log('Error getting latest relase', e);
      return null;
    }
  };

  return { getPackages, getReleases, getPackage, getLatestRelease };
}

interface UseNearProps {
  accountId: string | null;
  selector: WalletSelector;
}

interface HandleSignMessageProps {
  selector: WalletSelector;
  appName: string;
  setErrorMessage: (message: string) => void;
  showServerDownPopup: () => void;
}

export function useNear({ accountId, selector }: UseNearProps) {
  const navigate = useNavigate();

  const getAccount = useCallback(async (): Promise<Account | null> => {
    if (!accountId) {
      return null;
    }

    const { network } = selector.options;
    const provider = new providers.JsonRpcProvider({ url: network.nodeUrl });

    return provider
      .query<AccountView>({
        request_type: 'view_account',
        finality: 'final',
        account_id: accountId,
      })
      .then((data: any) => ({
        ...data,
        account_id: accountId,
      }));
  }, [accountId, selector]);

  const verifyMessage = useCallback(
    async (
      message: SignMessageParams,
      signedMessage: SignedMessage,
      setErrorMessage: (message: string) => void,
    ): Promise<boolean> => {
      try {
        const verifiedSignature = verifySignature({
          publicKey: signedMessage.publicKey,
          signature: signedMessage.signature,
          message: message.message,
          nonce: message.nonce,
          recipient: message.recipient,
          callbackUrl: message.callbackUrl ?? '',
        });
        const verifiedFullKeyBelongsToUser = await verifyFullKeyBelongsToUser({
          publicKey: signedMessage.publicKey,
          accountId: signedMessage.accountId,
          network: selector.options.network,
        });

        const isMessageVerified =
          verifiedFullKeyBelongsToUser && verifiedSignature;

        return isMessageVerified;
      } catch (error) {
        console.error(`${t.verifyMessageError}: ${error}`);
        setErrorMessage(t.verifyMessageError);
        return false;
      }
    },
    [selector.options.network],
  );

  const verifyMessageBrowserWallet = useCallback(
    async (
      isLogin: boolean,
      setErrorMessage: (message: string) => void,
      showServerDownPopup: () => void,
    ) => {
      const urlParams = new URLSearchParams(window.location.hash.substring(1));
      const accId = urlParams.get('accountId') as string;
      const publicKey = urlParams.get('publicKey') as string;
      const signature = urlParams.get('signature') as string;
      if (!accId && !publicKey && !signature) {
        console.error(t.missingUrlParamsError);
        return;
      }

      const message: SignMessageParams = JSON.parse(
        localStorage.getItem('message')!,
      );

      const state: SignatureMessageMetadata = JSON.parse(message.state!);

      if (!state.publicKey) {
        state.publicKey = publicKey;
      }

      const stateMessage: SignatureMessageMetadata = JSON.parse(state.message);
      if (!stateMessage.publicKey) {
        stateMessage.publicKey = publicKey;
        state.message = JSON.stringify(stateMessage);
      }

      const signedMessage = {
        accountId: accId,
        publicKey,
        signature,
      };

      const isMessageVerified: boolean = await verifyMessage(
        message,
        signedMessage,
        setErrorMessage,
      );

      const url = new URL(window.location.href);
      url.hash = '';
      url.search = '';
      window.history.replaceState({}, document.title, url);
      localStorage.removeItem('message');

      if (isMessageVerified) {
        const signatureMetadata: NearSignatureMessageMetadata = {
          recipient: message.recipient,
          callbackUrl: message.callbackUrl!,
          nonce: message.nonce.toString('base64'),
        };
        const payload: Payload = {
          message: state,
          metadata: signatureMetadata,
        };
        const walletSignatureData: WalletSignatureData = {
          payload: payload,
          publicKey: publicKey,
        };
        const walletMetadata: WalletMetadata = {
          wallet: WalletType.NEAR({
            networkId: selector.options.network.networkId,
          }),
          verifyingKey: publicKey,
          walletAddress: accId,
        };

        const nearRequest: LoginRequest = {
          walletSignature: signature,
          payload: walletSignatureData.payload!,
          walletMetadata: walletMetadata,
        };

        const result: ResponseData<LoginResponse> = isLogin
          ? await apiClient(showServerDownPopup).node().login(nearRequest)
          : await apiClient(showServerDownPopup).node().addRootKey(nearRequest);

        if (result.error) {
          const errorMessage = isLogin ? t.loginError : t.rootkeyError;
          console.error(`${errorMessage}: ${result.error.message}`);
          setErrorMessage(`${errorMessage}: ${result.error.message}`);
        } else {
          setStorageNodeAuthorized();
          navigate('/identity');
        }
      } else {
        console.error(t.messageNotVerifiedError);
        setErrorMessage(t.messageNotVerifiedError);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [verifyMessage],
  );

  async function handleSignMessage({
    selector,
    appName,
    setErrorMessage,
    showServerDownPopup,
  }: HandleSignMessageProps) {
    try {
      const challengeResponseData: ResponseData<NodeChallenge> =
        await apiClient(showServerDownPopup).node().requestChallenge();

      if (challengeResponseData.error) {
        return;
      }
      const { publicKey } = await getOrCreateKeypair();

      const wallet = await selector.wallet('my-near-wallet');

      const challengeNonce =
        challengeResponseData?.data?.nonce ?? randomBytes(32).toString('hex');

      const nonce: Buffer = Buffer.from(challengeNonce, 'base64');
      const recipient = appName;
      const callbackUrl = window.location.href;
      const nodeSignature = challengeResponseData.data?.nodeSignature ?? '';
      const timestamp =
        challengeResponseData.data?.timestamp ?? new Date().getTime();

      const signatureMessage: SignatureMessage = {
        nodeSignature,
        publicKey: publicKey,
      };
      const message: string = JSON.stringify(signatureMessage);

      const state: SignatureMessageMetadata = {
        publicKey: publicKey,
        nodeSignature,
        nonce: nonce.toString('base64'),
        timestamp,
        message,
      };

      if (wallet.type === 'browser') {
        localStorage.setItem(
          'message',
          JSON.stringify({
            message,
            nonce: [...nonce],
            recipient,
            callbackUrl,
            state: JSON.stringify(state),
          }),
        );
      }

      await wallet.signMessage({ message, nonce, recipient, callbackUrl });
    } catch (error) {
      console.error(`${t.signMessageError}: ${error}`);
      setErrorMessage(t.signMessageError);
    }
  }

  return { getAccount, handleSignMessage, verifyMessageBrowserWallet };
}

interface HandleSwitchAccountProps {
  accounts: AccountState[];
  accountId: string | null;
  selector: WalletSelector;
}

interface HandleSignoutProps {
  account: Account | null;
  selector: WalletSelector;
  setAccount: (account: Account | null) => void;
  setErrorMessage: (message: string) => void;
}

export const useWallet = () => {
  function handleSwitchWallet(modal: WalletSelectorModal) {
    modal.show();
  }

  async function handleSignOut({
    account,
    selector,
    setAccount,
    setErrorMessage,
  }: HandleSignoutProps) {
    if (!account) {
      return;
    }
    const wallet = await selector.wallet();

    wallet
      .signOut()
      .then(() => {
        setAccount(null);
      })
      .catch((err: any) => {
        setErrorMessage(t.signOutError);
        console.error(err);
      });
  }

  function handleSwitchAccount({
    accounts,
    accountId,
    selector,
  }: HandleSwitchAccountProps) {
    const currentIndex = accounts.findIndex(
      (x: AccountState) => x.accountId === accountId,
    );
    const nextIndex = currentIndex < accounts.length - 1 ? currentIndex + 1 : 0;

    const nextAccountId = accounts[nextIndex]?.accountId;

    selector.setActiveAccount(nextAccountId ?? '');
  }

  return { handleSwitchWallet, handleSwitchAccount, handleSignOut };
};
