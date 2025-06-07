import { AuthTokens } from '../types/api';

const ROOT_TOKEN_KEY = 'calimero_root_token';
const CLIENT_TOKEN_KEY = 'calimero_client_token';

export class AuthStorage {
    static setRootToken(token: string): void {
        localStorage.setItem(ROOT_TOKEN_KEY, token);
    }

    static getRootToken(): string | null {
        return localStorage.getItem(ROOT_TOKEN_KEY);
    }

    static setClientTokens(tokens: AuthTokens): void {
        localStorage.setItem(CLIENT_TOKEN_KEY, JSON.stringify(tokens));
    }

    static getClientTokens(): AuthTokens | null {
        const tokens = localStorage.getItem(CLIENT_TOKEN_KEY);
        return tokens ? JSON.parse(tokens) : null;
    }

    static clearTokens(): void {
        localStorage.removeItem(ROOT_TOKEN_KEY);
        localStorage.removeItem(CLIENT_TOKEN_KEY);
    }

    static hasValidRootToken(): boolean {
        const token = this.getRootToken();
        if (!token) return false;
        
        // TODO: Add JWT expiration check if needed
        return true;
    }

    static hasValidClientTokens(): boolean {
        const tokens = this.getClientTokens();
        if (!tokens) return false;

        // TODO: Add token expiration check if needed
        return true;
    }
} 