import React from 'react';
import Button from '../common/Button';
import {
  PermissionsContainer,
  PermissionsInfo,
  PermissionsList,
  PermissionsNotice,
  ButtonGroup
} from './styles';

interface PermissionsViewProps {
  permissions: string[];
  selectedContext: string;
  selectedIdentity: string;
  onComplete: (context: string, identity: string) => void;
  onBack: () => void;
}

export function PermissionsView({
  permissions,
  selectedContext,
  selectedIdentity,
  onComplete,
  onBack
}: PermissionsViewProps) {
  return (
    <PermissionsContainer>
      <PermissionsInfo>
        <h3>Review Permissions</h3>
        {permissions.length > 0 ? (
          <>
            <p>The application is requesting the following permissions:</p>
            <PermissionsList>
              {permissions.map((permission) => (
                <li key={permission}>{permission}</li>
              ))}
            </PermissionsList>
          </>
        ) : (
          <PermissionsNotice>
            The application will be granted default context permissions.
          </PermissionsNotice>
        )}
      </PermissionsInfo>
      <ButtonGroup>
        <Button onClick={onBack}>
          Back
        </Button>
        <Button
          onClick={() => onComplete(selectedContext, selectedIdentity)}
          primary
        >
          Approve
        </Button>
      </ButtonGroup>
    </PermissionsContainer>
  );
} 