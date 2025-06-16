import React, { useEffect, useState, useCallback } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import { useNavigate } from 'react-router-dom';
import PageContentWrapper from '../components/common/PageContentWrapper';
import IdentityTable from '../components/identity/IdentityTable';
import { apiClient } from '@calimero-network/calimero-client';
import {
  ClientKey,
  RootKey,
} from '@calimero-network/calimero-client/lib/api/adminApi';

type KeyType = 'root' | 'client';
type KeyStatus = 'active' | 'revoked';

export default function IdentityPage() {
  const navigate = useNavigate();
  const [errorMessage, setErrorMessage] = useState('');
  const [rootKeys, setRootKeys] = useState<RootKey[]>([]);
  const [clientKeys, setClientKeys] = useState<ClientKey[]>([]);
  const [keyType, setKeyType] = useState<KeyType>('root');
  const [keyStatus, setKeyStatus] = useState<KeyStatus>('active');

  const fetchRootKeys = useCallback(async () => {
    setErrorMessage('');
    const rootKeysResponse = await apiClient.admin().getRootKeys();
    if (rootKeysResponse.error) {
      setErrorMessage(rootKeysResponse.error.message);
      return;
    } else if (rootKeysResponse.data) {
      setRootKeys(rootKeysResponse.data);
    }
  }, []);

  const fetchClientKeys = useCallback(async () => {
    setErrorMessage('');
    const clientKeysResponse = await apiClient.admin().getClientKeys();
    if (clientKeysResponse.error) {
      setErrorMessage(clientKeysResponse.error.message);
      return;
    } else if (clientKeysResponse.data) {
      setClientKeys(clientKeysResponse.data);
    }
  }, []);

  useEffect(() => {
    if (keyType === 'root') {
      fetchRootKeys();
    } else {
      fetchClientKeys();
    }
  }, [keyType, fetchRootKeys, fetchClientKeys]);

  const activeRootKeys = rootKeys.filter((key) => !key.revoked_at);
  const revokedRootKeys = rootKeys.filter((key) => key.revoked_at);
  const activeClientKeys = clientKeys.filter((key) => !key.revoked_at);
  const revokedClientKeys = clientKeys.filter((key) => key.revoked_at);

  const currentKeys =
    keyType === 'root'
      ? keyStatus === 'active'
        ? activeRootKeys
        : revokedRootKeys
      : keyStatus === 'active'
        ? activeClientKeys
        : revokedClientKeys;

  const activeCount =
    keyType === 'root' ? activeRootKeys.length : activeClientKeys.length;
  const revokedCount =
    keyType === 'root' ? revokedRootKeys.length : revokedClientKeys.length;

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <IdentityTable
          onAddRootKey={() => navigate('/identity/root-key')}
          keysList={currentKeys}
          keyType={keyType}
          onKeyTypeChange={setKeyType}
          onCopyKeyClick={(publicKey: string) =>
            navigator.clipboard.writeText(publicKey)
          }
          errorMessage={errorMessage}
          keyStatus={keyStatus}
          onKeyStatusChange={setKeyStatus}
          activeKeysCount={activeCount}
          revokedKeysCount={revokedCount}
          onRefresh={keyType === 'root' ? fetchRootKeys : fetchClientKeys}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
