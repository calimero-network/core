import React, { useState } from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { ContentCard } from '../common/ContentCard';
import ListTable from '../common/ListTable';
import {
  RootKey,
  ClientKey,
} from '@calimero-network/calimero-client/lib/api/adminApi';
import ActionDialog from '../common/ActionDialog';
import { apiClient } from '@calimero-network/calimero-client';
import PermissionsDialog from './PermissionsDialog';
import MenuIconDropdown from '../common/MenuIconDropdown';

const TableWrapper = styled.div`
  display: flex;
  flex-direction: column;
  flex: 1;
  background-color: #17191b;
  border-radius: 0.5rem;
`;

const TabsContainer = styled.div`
  display: flex;
  gap: 1rem;
  padding: 1rem 1.5rem;
  border-bottom: 1px solid #23262d;
`;

const Tab = styled.button<{ $isActive: boolean }>`
  background: none;
  border: none;
  color: ${(props) => (props.$isActive ? '#4cfafc' : '#9c9da3')};
  font-size: 0.875rem;
  font-weight: 500;
  cursor: pointer;
  padding: 0.5rem 0;
  position: relative;

  &:after {
    content: '';
    position: absolute;
    bottom: -1px;
    left: 0;
    width: 100%;
    height: 2px;
    background-color: ${(props) =>
      props.$isActive ? '#4cfafc' : 'transparent'};
  }

  &:hover {
    color: #4cfafc;
  }
`;

const FlexWrapper = styled.div`
  flex: 1;
`;

const RowItem = styled.div`
  display: flex;
  align-items: center;
  padding: 0.75rem 1.5rem;
  border-top: 1px solid #23262d;
  font-size: 0.875rem;
  line-height: 1.25rem;

  .type {
    width: 25%;
    color: #fff;
    overflow-wrap: break-word;
  }

  .date {
    width: 25%;
    color: #fff;
  }

  .key {
    width: 40%;
    color: #9c9da3;
    word-break: break-all;
  }

  .actions {
    width: 10%;
    display: flex;
    justify-content: flex-end;
  }
`;

interface IdentityTableProps {
  keysList: (RootKey | ClientKey)[];
  keyType: 'root' | 'client';
  onKeyTypeChange: (type: 'root' | 'client') => void;
  onAddRootKey: () => void;
  onCopyKeyClick: (key: string) => void;
  errorMessage: string;
  keyStatus: 'active' | 'revoked';
  onKeyStatusChange: (status: 'active' | 'revoked') => void;
  activeKeysCount: number;
  revokedKeysCount: number;
  onRefresh: () => void;
}

export default function IdentityTable({
  keysList,
  keyType,
  onKeyTypeChange,
  onAddRootKey,
  onCopyKeyClick,
  errorMessage,
  keyStatus,
  onKeyStatusChange,
  activeKeysCount,
  revokedKeysCount,
  onRefresh,
}: IdentityTableProps) {
  const t = translations.identityPage;
  const [showRevokeDialog, setShowRevokeDialog] = useState(false);
  const [showPermissionsDialog, setShowPermissionsDialog] = useState(false);
  const [selectedKey, setSelectedKey] = useState<RootKey | ClientKey | null>(
    null,
  );
  const [revokeStatus, setRevokeStatus] = useState<
    'idle' | 'success' | 'error'
  >('idle');

  const handleRevoke = async (key: RootKey | ClientKey) => {
    setSelectedKey(key);
    setShowRevokeDialog(true);
  };

  const confirmRevoke = async () => {
    if (!selectedKey) return;

    let response;
    if ('public_key' in selectedKey) {
      // RootKey
      response = await apiClient.admin().revokeRootKey(selectedKey.key_id);
    } else {
      // ClientKey
      response = await apiClient
        .admin()
        .revokeClientKey(selectedKey.root_key_id, selectedKey.client_id);
    }

    if (response.error) {
      console.error('Error revoking key:', response.error);
      setRevokeStatus('error');
    } else {
      setRevokeStatus('success');
      onRefresh();
    }
    setShowRevokeDialog(false);
  };

  const getKeyIdentifier = (key: RootKey | ClientKey): string => {
    if ('public_key' in key) {
      return key.public_key;
    }
    return key.client_id;
  };

  const getKeyName = (key: RootKey | ClientKey): string => {
    if ('public_key' in key) {
      return key.auth_method;
    }
    return key.name || 'Unknown';
  };

  const handlePermissions = (key: RootKey | ClientKey) => {
    setSelectedKey(key);
    setShowPermissionsDialog(true);
  };

  const getKeyOptions = (key: RootKey | ClientKey) => {
    const options = [
      {
        title: 'Copy Key',
        onClick: () => onCopyKeyClick(getKeyIdentifier(key)),
      },
    ];

    if (keyStatus === 'active') {
      options.push(
        {
          title: 'Manage Permissions',
          onClick: () => handlePermissions(key),
        },
        {
          title: 'Revoke Key',
          onClick: () => handleRevoke(key),
        },
      );
    }

    return options;
  };

  const getTableHeaders = () => {
    if (keyType === 'root') {
      return ['AUTH METHOD', 'CREATED', 'PUBLIC KEY', ''];
    }
    return ['NAME', 'CREATED', 'CLIENT ID', ''];
  };

  return (
    <>
      <ContentCard
        headerTitle={'Current Identities'}
        headerSecondOptionText={t.addRootKeyText}
        headerOnSecondOptionClick={onAddRootKey}
        isOverflow={true}
      >
        <TableWrapper>
          <TabsContainer>
            <Tab
              $isActive={keyType === 'root'}
              onClick={() => onKeyTypeChange('root')}
            >
              Root Keys
            </Tab>
            <Tab
              $isActive={keyType === 'client'}
              onClick={() => onKeyTypeChange('client')}
            >
              Client Keys
            </Tab>
          </TabsContainer>
          <TabsContainer>
            <Tab
              $isActive={keyStatus === 'active'}
              onClick={() => onKeyStatusChange('active')}
            >
              Active ({activeKeysCount})
            </Tab>
            <Tab
              $isActive={keyStatus === 'revoked'}
              onClick={() => onKeyStatusChange('revoked')}
            >
              Revoked ({revokedKeysCount})
            </Tab>
          </TabsContainer>
          <FlexWrapper>
            <ListTable
              listHeaderItems={getTableHeaders()}
              numOfColumns={4}
              listItems={keysList}
              rowItem={(item, id, lastIndex) => (
                <RowItem key={id}>
                  <div className="type">{getKeyName(item)}</div>
                  <div className="date">
                    {new Date(item.created_at * 1000).toLocaleDateString()}
                  </div>
                  <div className="key">{getKeyIdentifier(item)}</div>
                  <div className="actions">
                    <MenuIconDropdown options={getKeyOptions(item)} />
                  </div>
                </RowItem>
              )}
              roundTopItem={true}
              noItemsText={
                keyStatus === 'active' ? 'No active keys' : 'No revoked keys'
              }
              error={errorMessage}
            />
          </FlexWrapper>
        </TableWrapper>
      </ContentCard>

      <ActionDialog
        show={showRevokeDialog}
        closeDialog={() => {
          setShowRevokeDialog(false);
          setRevokeStatus('idle');
        }}
        onConfirm={confirmRevoke}
        title={revokeStatus === 'success' ? 'Key Revoked' : 'Revoke Key'}
        subtitle={
          revokeStatus === 'success'
            ? 'The key has been successfully revoked.'
            : `Are you sure you want to revoke this key? This action cannot be undone.`
        }
        buttonActionText={revokeStatus === 'success' ? 'Close' : 'Revoke'}
      />

      <PermissionsDialog
        show={showPermissionsDialog}
        onClose={() => setShowPermissionsDialog(false)}
        selectedKey={selectedKey}
        onPermissionsUpdated={onRefresh}
      />
    </>
  );
}
