import React, { useState } from 'react';
import styled from 'styled-components';
import { ClientKey } from '../../../types/client-key';
import ListTable from '../../common/ListTable';
import clientKeyRowItem from './ClientKeyRowItem';
import translations from '../../../constants/en.global.json';
import ActionDialog from '../../common/ActionDialog';
import { apiClient } from '@calimero-network/calimero-client';

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
  border-bottom: 1px solid #23262D;
`;

const Tab = styled.button<{ $isActive: boolean }>`
  background: none;
  border: none;
  color: ${props => props.$isActive ? '#4cfafc' : '#9c9da3'};
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
    background-color: ${props => props.$isActive ? '#4cfafc' : 'transparent'};
  }

  &:hover {
    color: #4cfafc;
  }
`;

const FlexWrapper = styled.div`
  flex: 1;
`;

interface ClientKeysTableProps {
  clientKeys: ClientKey[];
  error: string | null;
  onKeyStatusChange?: (() => void) | undefined;
}

export default function ClientKeysTable({ clientKeys, error, onKeyStatusChange }: ClientKeysTableProps) {
  const [activeTab, setActiveTab] = useState<'active' | 'revoked'>('active');
  const [showRevokeDialog, setShowRevokeDialog] = useState(false);
  const [selectedKey, setSelectedKey] = useState<ClientKey | null>(null);
  const [revokeStatus, setRevokeStatus] = useState<'idle' | 'success' | 'error'>('idle');
  const t = translations.contextPage.contextDetails;

  const activeKeys = clientKeys.filter(key => !key.revoked_at);
  const revokedKeys = clientKeys.filter(key => key.revoked_at);

  const handleRevoke = (clientId: string) => {
    const key = clientKeys.find(k => k.client_id === clientId);
    if (key) {
      setSelectedKey(key);
      setShowRevokeDialog(true);
    }
  };

  const confirmRevoke = async () => {
    if (selectedKey) {
      const response = await apiClient.admin().revokeClientKey(selectedKey.root_key_id, selectedKey.client_id);
      if (response.error) {
        console.error('Error revoking client key:', response.error);
        setRevokeStatus('error');
      } else {
        setRevokeStatus('success');
        if (onKeyStatusChange) {
          onKeyStatusChange();
        }
      }
      setShowRevokeDialog(false);
    }
  };

  return (
    <>
      <TableWrapper>
        <TabsContainer>
          <Tab 
            $isActive={activeTab === 'active'} 
            onClick={() => setActiveTab('active')}
          >
            Active ({activeKeys.length})
          </Tab>
          <Tab 
            $isActive={activeTab === 'revoked'} 
            onClick={() => setActiveTab('revoked')}
          >
            Revoked ({revokedKeys.length})
          </Tab>
        </TabsContainer>
        <FlexWrapper>
          <ListTable<ClientKey>
            listDescription={t.clientKeysListDescription}
            numOfColumns={4}
            listHeaderItems={['NAME', 'ADDED', 'CLIENT ID', '']}
            listItems={activeTab === 'active' ? activeKeys : revokedKeys}
            error={error ?? ''}
            rowItem={(item, id, lastIndex) => 
              clientKeyRowItem(
                item,
                id,
                lastIndex,
                (clientId: string) => navigator.clipboard.writeText(clientId),
                activeTab === 'active' ? handleRevoke : undefined
              )
            }
            roundTopItem={true}
            noItemsText={activeTab === 'active' ? t.noClientKeysText : 'No revoked client keys'}
          />
        </FlexWrapper>
      </TableWrapper>

      <ActionDialog
        show={showRevokeDialog}
        closeDialog={() => {
          setShowRevokeDialog(false);
          setRevokeStatus('idle');
        }}
        onConfirm={confirmRevoke}
        title={revokeStatus === 'success' ? 'Client Key Revoked' : 'Revoke Client Key'}
        subtitle={
          revokeStatus === 'success'
            ? 'The client key has been successfully revoked.'
            : `Are you sure you want to revoke the client key ${selectedKey?.name}? This action cannot be undone.`
        }
        buttonActionText={revokeStatus === 'success' ? 'Close' : 'Revoke'}
      />
    </>
  );
} 