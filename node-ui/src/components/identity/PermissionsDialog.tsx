import React, { useState, useEffect } from 'react';
import styled from 'styled-components';
import Modal from 'react-bootstrap/Modal';
import {
  RootKey,
  ClientKey,
  UpdateKeyPermissionsRequest,
} from '@calimero-network/calimero-client/lib/api/adminApi';
import { apiClient } from '@calimero-network/calimero-client';
import translations from '../../constants/en.global.json';
import { ChevronDown, ChevronRight } from 'react-feather';

const ModalWrapper = styled.div`
  background-color: #212325;
  border-radius: 0.5rem;
  padding: 1.5rem;
  color: #fff;
  max-height: 80vh;
  overflow-y: auto;
`;

const Title = styled.h2`
  font-size: 1.25rem;
  margin-bottom: 1.5rem;
`;

const PermissionList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  margin-bottom: 1.5rem;
`;

const PermissionSection = styled.div`
  border: 1px solid #2d3035;
  border-radius: 0.5rem;
  overflow: hidden;
`;

const SectionHeader = styled.div`
  display: flex;
  align-items: center;
  padding: 1rem;
  background-color: #2d3035;
  cursor: pointer;
  user-select: none;

  &:hover {
    background-color: #363a40;
  }
`;

const SectionTitle = styled.div`
  display: flex;
  align-items: center;
  gap: 0.75rem;
  flex: 1;
  font-weight: 500;
`;

const SectionContent = styled.div<{ $isOpen: boolean }>`
  display: ${(props) => (props.$isOpen ? 'block' : 'none')};
  padding: 1rem;
  background-color: #282a2d;
`;

const PermissionItem = styled.div`
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 0.5rem;

  input[type='checkbox'] {
    width: 1rem;
    height: 1rem;
    cursor: pointer;
  }

  label {
    font-size: 0.875rem;
    cursor: pointer;
    flex: 1;
  }
`;

const ButtonGroup = styled.div`
  display: flex;
  justify-content: flex-end;
  gap: 1rem;
  margin-top: 1rem;
`;

const Button = styled.button<{ $primary?: boolean }>`
  padding: 0.5rem 1rem;
  border-radius: 0.25rem;
  border: none;
  font-size: 0.875rem;
  cursor: pointer;
  background-color: ${(props) => (props.$primary ? '#4cfafc' : '#2d3035')};
  color: ${(props) => (props.$primary ? '#000' : '#fff')};

  &:hover {
    opacity: 0.9;
  }
`;

const ErrorMessage = styled.div`
  color: #ff4d4d;
  font-size: 0.875rem;
  margin-bottom: 1rem;
`;

interface PermissionsDialogProps {
  show: boolean;
  onClose: () => void;
  selectedKey: RootKey | ClientKey | null;
  onPermissionsUpdated: () => void;
}

type KeyWithPermissions = {
  permissions: string[];
  key_id: string;
};

interface PermissionGroup {
  id: string;
  label: string;
  description?: string;
  permissions: {
    id: string;
    label: string;
    description?: string;
    parameterized?: boolean;
  }[];
}

interface ContextSpecificPermission {
  contextId: string;
  userId: string;
  fullPermission: string;
}

const PERMISSION_GROUPS: PermissionGroup[] = [
  {
    id: 'admin',
    label: 'Admin Access',
    description: 'Full administrative access to all features',
    permissions: [
      {
        id: 'admin',
        label: 'Do anything',
        description: 'Complete unrestricted access',
      },
    ],
  },
  {
    id: 'application',
    label: 'Applications',
    permissions: [
      { id: 'application', label: 'Full Application Access' },
      { id: 'application:list', label: 'List All Applications' },
      { id: 'application:install', label: 'Install Applications' },
      { id: 'application:uninstall', label: 'Uninstall Applications' },
    ],
  },
  {
    id: 'blob',
    label: 'Blobs',
    permissions: [
      { id: 'blob', label: 'Full Blob Access' },
      { id: 'blob:add', label: 'Add Blobs' },
      { id: 'blob:add:stream', label: 'Add Blobs from Streams' },
      { id: 'blob:add:file', label: 'Add Blobs from Files' },
      { id: 'blob:add:url', label: 'Add Blobs from URLs' },
      { id: 'blob:remove', label: 'Remove Blobs' },
    ],
  },
  {
    id: 'context',
    label: 'Contexts',
    permissions: [
      { id: 'context', label: 'Full Context Access' },
      { id: 'context:create', label: 'Create Contexts' },
      { id: 'context:list', label: 'List Contexts' },
      { id: 'context:delete', label: 'Delete Contexts' },
      { id: 'context:leave', label: 'Leave Contexts' },
      { id: 'context:invite', label: 'Invite to Contexts' },
      { id: 'context:execute', label: 'Execute in Contexts' },
      { id: 'context:alias:create', label: 'Create Context Aliases' },
      { id: 'context:alias:delete', label: 'Delete Context Aliases' },
      { id: 'context:alias:lookup', label: 'Lookup Context Aliases' },
    ],
  },
  {
    id: 'keys',
    label: 'Keys',
    permissions: [
      { id: 'keys', label: 'Full Key Access' },
      { id: 'keys:create', label: 'Create Keys' },
      { id: 'keys:list', label: 'List Keys' },
      { id: 'keys:delete', label: 'Delete Keys' },
    ],
  },
];

const ContextPermissionSection = styled(PermissionSection)`
  margin-top: 1rem;
  background-color: #282a2d;
`;

const ContextPermissionHeader = styled.div`
  padding: 1rem;
  border-bottom: 1px solid #2d3035;
`;

const ContextPermissionDetails = styled.div`
  padding: 0.5rem 1rem;
  font-size: 0.875rem;
  color: #999;
`;

export default function PermissionsDialog({
  show,
  onClose,
  selectedKey,
  onPermissionsUpdated,
}: PermissionsDialogProps) {
  const t = translations.keysTable;
  const [selectedPermissions, setSelectedPermissions] = useState<string[]>([]);
  const [contextSpecificPermissions, setContextSpecificPermissions] = useState<
    ContextSpecificPermission[]
  >([]);
  const [error, setError] = useState<string>('');
  const [isLoading, setIsLoading] = useState(false);
  const [openSections, setOpenSections] = useState<string[]>([]);

  const parseContextPermission = (
    permission: string,
  ): ContextSpecificPermission | null => {
    const match = permission.match(/^context\[([\w-]+),([\w-]+)\]$/);
    if (match && match[1] && match[2]) {
      return {
        contextId: match[1],
        userId: match[2],
        fullPermission: permission,
      };
    }
    return null;
  };

  useEffect(() => {
    if (selectedKey) {
      const key = selectedKey as unknown as KeyWithPermissions;
      const permissions = key.permissions || [];

      // Separate context-specific permissions from regular permissions
      const { contextPerms, regularPerms } = permissions.reduce(
        (acc, permission) => {
          const contextPerm = parseContextPermission(permission);
          if (contextPerm) {
            acc.contextPerms.push(contextPerm);
          } else {
            acc.regularPerms.push(permission);
          }
          return acc;
        },
        {
          contextPerms: [] as ContextSpecificPermission[],
          regularPerms: [] as string[],
        },
      );

      setSelectedPermissions(regularPerms);
      setContextSpecificPermissions(contextPerms);

      // Open sections that have selected permissions
      const sectionsToOpen = PERMISSION_GROUPS.filter(
        (group) =>
          regularPerms.some((p) => p.startsWith(group.id)) ||
          (contextPerms.length > 0 && group.id === 'context'),
      ).map((group) => group.id);
      setOpenSections(sectionsToOpen);
    }
  }, [selectedKey]);

  const toggleSection = (sectionId: string) => {
    setOpenSections((prev) =>
      prev.includes(sectionId)
        ? prev.filter((id) => id !== sectionId)
        : [...prev, sectionId],
    );
  };

  const handlePermissionToggle = (permission: string) => {
    setSelectedPermissions((prev) => {
      const newPermissions = prev.includes(permission)
        ? prev.filter((p) => p !== permission)
        : [...prev, permission];

      if (
        newPermissions.length === 0 &&
        contextSpecificPermissions.length === 0
      ) {
        setError(t.permissionsAtLeastOne);
        return prev;
      }

      setError('');
      return newPermissions;
    });
  };

  const handleSave = async () => {
    if (!selectedKey) return;

    setIsLoading(true);
    setError('');

    try {
      // Get the key_id from the selectedKey object
      const keyId =
        (selectedKey as any).client_id || (selectedKey as any).key_id;
      if (!keyId) {
        throw new Error('No key ID found');
      }

      const currentPermissions = (selectedKey as any).permissions || [];

      // Combine regular and context-specific permissions
      const allSelectedPermissions = [
        ...selectedPermissions,
        ...contextSpecificPermissions.map((p) => p.fullPermission),
      ];

      const toAdd = allSelectedPermissions.filter(
        (p: string) => !currentPermissions.includes(p),
      );
      const toRemove = currentPermissions.filter(
        (p: string) => !allSelectedPermissions.includes(p),
      );

      const request: UpdateKeyPermissionsRequest = {};
      if (toAdd.length > 0) request.add = toAdd;
      if (toRemove.length > 0) request.remove = toRemove;

      const response = await apiClient
        .admin()
        .setKeyPermissions(keyId, request);

      if (response.error) {
        setError(response.error.message);
      } else {
        onPermissionsUpdated();
        onClose();
      }
    } catch (err: any) {
      setError(err.message || t.permissionsError);
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Modal show={show} onHide={onClose} centered size="lg">
      <ModalWrapper>
        <Title>{t.permissionsDialogTitle}</Title>
        {error && <ErrorMessage>{error}</ErrorMessage>}
        <PermissionList>
          {PERMISSION_GROUPS.map((group) => (
            <PermissionSection key={group.id}>
              <SectionHeader onClick={() => toggleSection(group.id)}>
                <SectionTitle>
                  {openSections.includes(group.id) ? (
                    <ChevronDown size={20} />
                  ) : (
                    <ChevronRight size={20} />
                  )}
                  <input
                    type="checkbox"
                    checked={selectedPermissions.includes(group.id)}
                    onChange={() => handlePermissionToggle(group.id)}
                  />
                  {group.label}
                </SectionTitle>
              </SectionHeader>
              <SectionContent $isOpen={openSections.includes(group.id)}>
                {group.permissions.map((permission) => (
                  <PermissionItem key={permission.id}>
                    <input
                      type="checkbox"
                      id={permission.id}
                      checked={selectedPermissions.includes(permission.id)}
                      onChange={() => handlePermissionToggle(permission.id)}
                    />
                    <label htmlFor={permission.id}>
                      {permission.label}
                      {permission.description && (
                        <div style={{ fontSize: '0.75rem', color: '#999' }}>
                          {permission.description}
                        </div>
                      )}
                    </label>
                  </PermissionItem>
                ))}
              </SectionContent>
            </PermissionSection>
          ))}

          {contextSpecificPermissions.length > 0 && (
            <ContextPermissionSection>
              <ContextPermissionHeader>
                <h3 style={{ margin: 0, fontSize: '1rem' }}>
                  Context-Specific Permissions
                </h3>
              </ContextPermissionHeader>
              {contextSpecificPermissions.map((permission, index) => (
                <ContextPermissionDetails key={index}>
                  Full access to context <strong>{permission.contextId}</strong>{' '}
                  as user <strong>{permission.userId}</strong>
                </ContextPermissionDetails>
              ))}
            </ContextPermissionSection>
          )}
        </PermissionList>
        <ButtonGroup>
          <Button onClick={onClose}>{t.permissionsCancelButton}</Button>
          <Button $primary onClick={handleSave} disabled={isLoading}>
            {isLoading ? 'Saving...' : t.permissionsSaveButton}
          </Button>
        </ButtonGroup>
      </ModalWrapper>
    </Modal>
  );
}
