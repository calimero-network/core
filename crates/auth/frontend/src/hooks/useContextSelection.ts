import { useState, useCallback } from 'react';
// import { Context, ContextIdentity } from '../types/api';
import { apiClient, getAccessToken, getRefreshToken } from '@calimero-network/calimero-client';
import { Context } from '@calimero-network/calimero-client/lib/api/nodeApi';

export function useContextSelection() {
    const [contexts, setContexts] = useState<Context[]>([]);
    const [selectedContext, setSelectedContext] = useState<string | null>(null);
    const [identities, setIdentities] = useState<string[]>([]);
    const [selectedIdentity, setSelectedIdentity] = useState<string | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Fetch available contexts
    const fetchContexts = useCallback(async () => {
        if (!getAccessToken() && !getRefreshToken()) {
            setError('No valid root token available');
            return;
        }

        try {
            setLoading(true);
            setError(null);
            const response = await apiClient.node().getContexts();
            if (response.error) {
                setError(response.error.message);
                return;
            }
            setContexts(response.data.contexts);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch contexts');
        } finally {
            setLoading(false);
        }
    }, []);

    // Fetch identities for selected context
    const fetchIdentities = useCallback(async (contextId: string) => {
        if (!getAccessToken() && !getRefreshToken()) {
            setError('No valid root token available');
            return;
        }

        try {
            setLoading(true);
            setError(null);
            const response = await apiClient.node().fetchContextIdentities(contextId);
            if (response.error) {
                setError(response.error.message);
                return;
            }
            setIdentities(response.data.identities);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch identities');
        } finally {
            setLoading(false);
        }
    }, []);

    // Handle context selection
    const handleContextSelect = useCallback(async (contextId: string | null) => {
        setSelectedContext(contextId);
        setSelectedIdentity(null);
        if (contextId) {
            await fetchIdentities(contextId);
        }
    }, [fetchIdentities]);

    // Handle identity selection
    const handleIdentitySelect = useCallback((identity: string | null) => {
        setSelectedIdentity(identity);
    }, []);

    // Reset selections
    const reset = useCallback(() => {
        setSelectedContext(null);
        setSelectedIdentity(null);
        setIdentities([]);
        setError(null);
    }, []);

    return {
        contexts,
        selectedContext,
        identities,
        selectedIdentity,
        loading,
        error,
        fetchContexts,
        handleContextSelect,
        handleIdentitySelect,
        reset,
    };
} 