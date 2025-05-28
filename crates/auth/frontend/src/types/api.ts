export interface Context {
    id: string;
    applicationId: string;
    rootHash: string;
}

// Identity is now just a string ID
export type ContextIdentity = string;

export interface GetContextsResponse {
    data: {
        contexts: Context[];
    };
}

export interface GetContextIdentitiesResponse {
    data: {
        identities: ContextIdentity[];
    };
}

export interface AuthTokens {
    access_token: string;
    refresh_token: string;
    token_type: string;
    expires_in: number;
    client_id: string;
} 