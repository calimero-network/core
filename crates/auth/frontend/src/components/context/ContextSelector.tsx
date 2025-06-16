import React, { useEffect, useMemo } from 'react';
import { useContextSelection } from '../../hooks/useContextSelection';
import { useContextCreation } from '../../hooks/useContextCreation';
import { getStoredUrlParam } from '../../utils/urlParams';
import './ContextSelector.css';
import Button from '../common/Button';
import { SelectContext, SelectContextIdentity } from '@calimero-network/calimero-client';

interface ContextSelectorProps {
    onComplete: (context: string, identity: string) => void;
    onBack: () => void;
}

export function ContextSelector({ onComplete, onBack }: ContextSelectorProps) {
    const {
        contexts,
        selectedContext,
        identities,
        selectedIdentity,
        loading: selectionLoading,
        error: selectionError,
        fetchContexts,
        handleContextSelect,
        handleIdentitySelect,
    } = useContextSelection();

    const {
        isLoading: creationLoading,
        error: creationError,
        createContext,
    } = useContextCreation();

    const permissions = useMemo(() => {
        const permissionsParam = getStoredUrlParam('permissions');
        return permissionsParam ? permissionsParam.split(',') : [];
    }, []);

    useEffect(() => {
        fetchContexts();
    }, [fetchContexts]);

    // Filter contexts based on applicationId URL parameter
    const applicationId = getStoredUrlParam('applicationId');
    const applicationPath = getStoredUrlParam('applicationPath');
    
    const filteredContexts = useMemo(() => {
        if (!applicationId) return contexts;
        return contexts.filter(context => context.applicationId === applicationId);
    }, [contexts, applicationId]);

    const loading = selectionLoading || creationLoading;
    const error = selectionError || creationError;

    if (loading) {
        return (
            <div className="context-selector">
                <div className="loading">Loading...</div>
            </div>
        );
    }

    if (error) {
        return (
            <div className="context-selector">
                <div className="error">{error}</div>
            </div>
        );
    }

    

    // No contexts available and applicationPath is present - show create context prompt
    if (!filteredContexts.length && applicationId && applicationPath) {
        return (
            <div className="context-selector">
                <div className="empty-state">
                    <h2>Create New Context</h2>
                    <p>There are no contexts for this application ID. Would you like to create a new context?</p>
                    <Button 
                        onClick={createContext}
                        disabled={loading}
                        primary
                    >
                        {loading ? 'Creating...' : 'Create New Context'}
                    </Button>
                </div>
            </div>
        );
    }

    // Context selection view
    return (
        <div className="context-selector">
            {
              !selectedContext && (
                <>
                  <h2>Select Context</h2>
                  <SelectContext
                      contextList={filteredContexts}
                      setSelectedContextId={(id) => {
                          handleContextSelect(id);
                      }}
                      backStep={onBack}
                  />
                </>
              )
            }
            {/* Identity Selection */}
            {selectedContext && identities.length > 0 && !selectedIdentity && (
              <SelectContextIdentity
                  contextIdentities={identities}
                  selectedContextId={selectedContext}
                  onSelectIdentity={handleIdentitySelect}
                  backStep={() => {
                      handleContextSelect(null);
                  }}
              />
            )}

            {/* Permissions Information */}
            {selectedContext && selectedIdentity && (
              <>
                <div className="permissions-info">
                    {permissions.length > 0 ? (
                        <>
                            <h3>Requested Permissions</h3>
                            <p>This application is requesting the following permissions:</p>
                            <ul className="permissions-list">
                                {permissions.map((permission, index) => (
                                    <li key={index}>{permission}</li>
                                ))}
                            </ul>
                            <p className="permissions-notice">
                                By continuing, you agree to grant these permissions to the application.
                            </p>
                        </>
                    ) : (
                        <>
                            <h3>Default Permissions</h3>
                            <p className="permissions-notice">
                                This application will be granted default context permissions.
                            </p>
                        </>
                    )}
                </div>
                <Button
                    onClick={() => onComplete(selectedContext, selectedIdentity)}
                    primary
                >
                    {permissions.length > 0 ? 'Continue and Grant Permissions' : 'Continue with Default Permissions'}
                </Button>
                <Button
                    onClick={() => {
                        handleIdentitySelect(null);
                    }}
                    primary
                >
                    Back
                </Button>
              </>
            )}
        </div>
    );
} 