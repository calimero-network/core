import { GetContextsResponse, GetContextIdentitiesResponse } from '../types/api';
import { AuthStorage } from './authStorage';

class CalimeroApi {
    private readonly baseUrl: string;

    constructor() {
        // Point to Calimero Node
        this.baseUrl = 'http://localhost:2428/admin-api';
    }

    private async request<T>(path: string, token?: string): Promise<T> {
        // Use provided token or get from storage
        const authToken = token || AuthStorage.getRootToken();
        
        if (!authToken) {
            throw new Error('No authentication token available');
        }

        const response = await fetch(`${this.baseUrl}${path}`, {
            headers: {
                'Authorization': `Bearer ${authToken}`,
                'Content-Type': 'application/json',
            },
        });

        if (!response.ok) {
            throw new Error(`API request failed: ${response.statusText}`);
        }

        return response.json();
    }

    async getContexts(token?: string): Promise<GetContextsResponse> {
        return this.request<GetContextsResponse>('/contexts', token);
    }

    async getContextIdentities(contextId: string, token?: string): Promise<GetContextIdentitiesResponse> {
        return this.request<GetContextIdentitiesResponse>(`/contexts/${contextId}/identities`, token);
    }
}

export const calimeroApi = new CalimeroApi(); 