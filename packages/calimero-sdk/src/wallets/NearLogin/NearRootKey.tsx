import React, { Fragment, useCallback, useEffect, useState } from 'react';
import { randomBytes } from 'crypto';
import { providers } from 'near-api-js';
import type { AccountView } from 'near-api-js/lib/providers/provider';
import {
  SignedMessage,
  verifyFullKeyBelongsToUser,
  verifySignature,
  type SignMessageParams,
} from '@near-wallet-selector/core';

import { useWalletSelector } from './WalletSelectorContext';
import apiClient from '../../api';
import { ResponseData } from '../../types/api-response';
import { setStorageNodeAuthorized } from '../../storage/storage';
import { Loading } from '../loading/Loading';
import {
  RootKeyRequest,
  NodeChallenge,
  WalletType,
  SignatureMessageMetadata,
  NearSignatureMessageMetadata,
  Payload,
  WalletMetadata,
  SignatureMessage,
} from '../../api/nodeApi';
import { getOrCreateKeypair } from '../../auth/ed25519';

type Account = AccountView & {
  account_id: string;
};

interface NearRootKeyProps {
  rpcBaseUrl: string;
  contextId?: string;
  successRedirect: () => void;
  cardBackgroundColor: string | undefined;
  nearTitleColor: string | undefined;
  navigateBack: () => void | undefined;
}

export const NearRootKey: React.FC<NearRootKeyProps> = ({
  rpcBaseUrl,
  contextId,
  successRedirect,
  cardBackgroundColor,
  nearTitleColor,
  navigateBack,
}) => {
  const { selector, accounts, modal, accountId } = useWalletSelector();
  const [account, setAccount] = useState<Account | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const appName = 'me';

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

    wallet
      .signOut()
      .then(() => {
        setAccount(null);
      })
      .catch((err: any) => {
        console.log('Failed to sign out');
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

    alert('Switched account to ' + nextAccountId);
  }

  const verifyMessage = useCallback(
    async (
      message: SignMessageParams,
      signedMessage: SignedMessage,
    ): Promise<boolean> => {
      console.log('verifyMessage', { message, signedMessage });

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
        ? 'Successfully verified'
        : 'Failed to verify';

      console.log(
        `${resultMessage} signed message: '${
          message.message
        }': \n ${JSON.stringify(signedMessage)}`,
      );

      return isMessageVerified;
    },
    [selector.options.network],
  );

  const verifyMessageBrowserWallet = useCallback(async () => {
    const urlParams = new URLSearchParams(window.location.hash.substring(1));
    const accId = urlParams.get('accountId') as string;
    const publicKey = urlParams.get('publicKey') as string;
    const signature = urlParams.get('signature') as string;

    if (!accId && !publicKey && !signature) {
      console.error('Missing params in url.');
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
      const walletMetadata: WalletMetadata = {
        wallet: WalletType.NEAR({ networkId: selector.options.network.networkId }),
        signingKey: publicKey,
      };
      const rootKeyRequest: RootKeyRequest = {
        walletSignature: signature,
        payload: payload,
        walletMetadata: walletMetadata,
        contextId: contextId,
      };

      await apiClient
        .node()
        .addRootKey(rootKeyRequest, rpcBaseUrl, contextId)
        .then((result) => {
          console.log('result', result);
          if (result.error) {
            console.error('Root key error', result.error);
          } else {
            setStorageNodeAuthorized();
            successRedirect();
            console.log('root key added');
          }
        })
        .catch(() => {
          console.error('error while adding root key');
        });
    } else {
      //TODO handle error
      console.error('Message not verified');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [verifyMessage]);

  async function handleSignMessage() {
    const challengeResponseData: ResponseData<NodeChallenge> = await apiClient
      .node()
      .requestChallenge(rpcBaseUrl, contextId as string);

    if (challengeResponseData.error) {
      console.log('requestChallenge api error', challengeResponseData.error);
      return;
    }

    const { publicKey } = await getOrCreateKeypair();

    const wallet = await selector.wallet('my-near-wallet');

    const challengeNonce =
      challengeResponseData?.data?.nonce ?? randomBytes(32).toString('hex');

    const nonce: Buffer = Buffer.from(challengeNonce, 'base64');
    const recipient = appName;
    const callbackUrl = window.location.href;
    const challengeContextId = challengeResponseData.data?.contextId ?? null;
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
      contextId: challengeContextId,
      timestamp,
      message,
    };

    if (wallet.type === 'browser') {
      console.log('browser');

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
  }

  if (loading) {
    return <Loading />;
  }

  return (
    <Fragment>
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          backgroundColor: cardBackgroundColor ?? '#1C1C1C',
          width: 'fit-content',
          padding: '2.5rem',
          gap: '1rem',
          borderRadius: '0.5rem',
          maxWidth: '400px',
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
            color: nearTitleColor ?? '#fff',
          }}
        >
          Add root key with NEAR
        </span>
        <div
          style={{
            display: 'flex',
            justifyContent: 'start',
            alignItems: 'center',
            fontSize: '14px',
            color: '#778899',
            whiteSpace: 'break-spaces'
          }}
        >
          <span>Choose which account from your wallet you want to add a node root key for.
          Each key, and therefore each account, can only be added once</span>
        </div>
        {account && (
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'center',
              backgroundColor: '#25282D',
              height: '73px',
              borderRadius: '6px',
              border: 'none',
              outline: 'none',
              paddingLeft: '12px',
              paddingRight: '12px',
              paddingTop: '4px',
              paddingBottom: '4px',
              width: '100%',
            }}
          >
            <div
              style={{
                display: 'flex',
                justifyContent: 'center',
                alignItems: 'center',
              }}
            >
              <div
                style={{
                  borderRadius: '50px',
                  display: 'inline-block',
                  margin: '0px',
                  overflow: 'hidden',
                  padding: '0px',
                  backgroundColor: 'rgb(241, 153, 2)',
                  height: '30px',
                  width: '30px',
                }}
              >
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  x="0"
                  y="0"
                  height="30"
                  width="30"
                >
                  <rect
                    x="0"
                    y="0"
                    rx="0"
                    ry="0"
                    height="30"
                    width="30"
                    transform="translate(-7.426761426750906 -1.7430703750826357) rotate(207.0 15 15)"
                    fill="#f3be00"
                  ></rect>
                  <rect
                    x="0"
                    y="0"
                    rx="0"
                    ry="0"
                    height="30"
                    width="30"
                    transform="translate(-3.5186766166074457 16.760741981511913) rotate(268.5 15 15)"
                    fill="#159ef2"
                  ></rect>
                  <rect
                    x="0"
                    y="0"
                    rx="0"
                    ry="0"
                    height="30"
                    width="30"
                    transform="translate(1.9965853361857855 -24.64656453889279) rotate(423.5 15 15)"
                    fill="#f73a01"
                  ></rect>
                  <rect
                    x="0"
                    y="0"
                    rx="0"
                    ry="0"
                    height="30"
                    width="30"
                    transform="translate(-13.721802107758117 35.84394280404586) rotate(187.7 15 15)"
                    fill="#c71450"
                  ></rect>
                </svg>
              </div>
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  paddingLeft: '1rem',
                }}
              >
                <span
                  style={{
                    color: '#fff',
                    fontSize: '13px',
                    lineHeight: '18px',
                    height: '19.5px',
                    fontWeight: 'bold',
                  }}
                >
                  Account Id
                </span>
                <span
                  style={{
                    color: '#fff',
                    fontSize: '11px',
                    height: '16.5px',
                    fontWeight: '500',
                  }}
                >
                  {accountId}
                </span>
              </div>
            </div>
            <div
              style={{
                backgroundColor: 'hsla(0, 0%, 100%, .05)',
                color: '#fff',
                cursor: 'pointer',
                padding: '8px',
                borderRadius: '4px',
              }}
              onClick={() => {
                if (account) {
                  handleSignOut();
                }
              }}
            >
              Logout
            </div>
          </div>
        )}
        <div
          style={{
            display: 'flex',
            marginTop: account ? '155px' : '12px',
            gap: '1rem',
          }}
        >
          <button
            style={{
              backgroundColor: '#FF7A00',
              color: 'white',
              width: 'fit-content',
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
            onClick={handleSwitchWallet}
          >
            Switch Wallet
          </button>
          <button
            style={{
              backgroundColor: '#FF7A00',
              color: 'white',
              width: 'fit-content',
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
            onClick={handleSignMessage}
          >
            Add root key
          </button>
          {accounts.length > 1 && (
            <button
              style={{
                backgroundColor: '#FF7A00',
                color: 'white',
                width: 'fit-content',
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
              onClick={handleSwitchAccount}
            >
              Switch Account
            </button>
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
      </div>
    </Fragment>
  );
};
