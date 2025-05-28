import { useState, useCallback } from 'react';
import { Context, ContextIdentity } from '../types/api';
import { calimeroApi } from '../services/calimeroApi';
import { AuthStorage } from '../services/authStorage';

export function useContextSelection() {
    const [contexts, setContexts] = useState<Context[]>([]);
    const [selectedContext, setSelectedContext] = useState<Context | null>(null);
    const [identities, setIdentities] = useState<ContextIdentity[]>([]);
    const [selectedIdentity, setSelectedIdentity] = useState<ContextIdentity | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Fetch available contexts
    const fetchContexts = useCallback(async () => {
        if (!AuthStorage.hasValidRootToken()) {
            setError('No valid root token available');
            return;
        }

        try {
            setLoading(true);
            setError(null);
            const response = await calimeroApi.getContexts();
            setContexts(response.data.contexts);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch contexts');
        } finally {
            setLoading(false);
        }
    }, []);

    // Fetch identities for selected context
    const fetchIdentities = useCallback(async (contextId: string) => {
        if (!AuthStorage.hasValidRootToken()) {
            setError('No valid root token available');
            return;
        }

        try {
            setLoading(true);
            setError(null);
            const response = await calimeroApi.getContextIdentities(contextId);
            setIdentities(response.data.identities);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to fetch identities');
        } finally {
            setLoading(false);
        }
    }, []);

    // Handle context selection
    const handleContextSelect = useCallback(async (context: Context) => {
        setSelectedContext(context);
        setSelectedIdentity(null);
        await fetchIdentities(context.id);
    }, [fetchIdentities]);

    // Handle identity selection
    const handleIdentitySelect = useCallback((identity: ContextIdentity) => {
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