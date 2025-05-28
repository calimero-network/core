import React, { useEffect } from 'react';
import { useContextSelection } from '../hooks/useContextSelection';
import { Context, ContextIdentity } from '../types/api';
import './ContextSelector.css';

interface ContextSelectorProps {
    onComplete: (context: Context, identity: ContextIdentity) => void;
}

export function ContextSelector({ onComplete }: ContextSelectorProps) {
    const {
        contexts,
        selectedContext,
        identities,
        selectedIdentity,
        loading,
        error,
        fetchContexts,
        handleContextSelect,
        handleIdentitySelect,
    } = useContextSelection();

    useEffect(() => {
        fetchContexts();
    }, [fetchContexts]);

    if (loading) {
        return <div className="context-selector">
            <div className="loading">Loading...</div>
        </div>;
    }

    if (error) {
        return <div className="context-selector">
            <div className="error">{error}</div>
        </div>;
    }

    if (!contexts.length) {
        return (
            <div className="context-selector">
                <div className="empty-state">
                    <h2>No Contexts Available</h2>
                    <p>There are currently no contexts available for selection. Please set up a context.</p>
                </div>
            </div>
        );
    }

    return (
        <div className="context-selector">
            <h2>Select Context</h2>
            
            {/* Context Selection */}
            <div className="context-list">
                {contexts.map(context => (
                    <div
                        key={context.id}
                        className={`context-item ${selectedContext?.id === context.id ? 'selected' : ''}`}
                        onClick={() => handleContextSelect(context)}
                    >
                        <h3>Context ID</h3>
                        <p>{context.id}</p>
                        <p>Application ID: {context.applicationId}</p>
                        <p>Root Hash: {context.rootHash}</p>
                    </div>
                ))}
            </div>

            {/* Identity Selection */}
            {selectedContext && identities.length > 0 && (
                <div className="identity-selection">
                    <h3>Select Identity for Context {selectedContext.id}</h3>
                    <div className="identity-list">
                        {identities.map(identity => (
                            <div
                                key={identity}
                                className={`identity-item ${selectedIdentity === identity ? 'selected' : ''}`}
                                onClick={() => handleIdentitySelect(identity)}
                            >
                                <h4>Identity</h4>
                                <p className="identity-id">{identity}</p>
                            </div>
                        ))}
                    </div>
                </div>
            )}

            {/* Complete Selection Button */}
            {selectedContext && selectedIdentity && (
                <button
                    className="complete-button"
                    onClick={() => onComplete(selectedContext, selectedIdentity)}
                >
                    Continue with Selected Context and Identity
                </button>
            )}
        </div>
    );
} 