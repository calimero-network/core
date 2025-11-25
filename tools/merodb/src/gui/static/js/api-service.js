/**
 * API Service Module
 * Handles all HTTP communication with the backend server
 * @module api-service
 */

export class ApiService {
    /**
     * Load database and optionally extract ABI from WASM file
     * @param {string} dbPath - Path to the database directory
     * @param {File|null} wasmFile - Optional WASM file for ABI extraction
     * @returns {Promise<{data: Object, warning?: string, info?: string}>}
     * @throws {Error} If the request fails or database doesn't exist
     */
    static async loadDatabase(dbPath, wasmFile) {
        const formData = new FormData();
        formData.append('db_path', dbPath);

        if (wasmFile) {
            formData.append('wasm_file', wasmFile);
        }

        const response = await fetch('/api/export', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to load database');
        }

        return response.json();
    }

    /**
     * List all available contexts (lightweight, doesn't build trees)
     * @param {string} dbPath - Path to the database directory
     * @returns {Promise<{data: {contexts: Array, total_contexts: number}}>}
     * @throws {Error} If the request fails
     */
    static async listContexts(dbPath) {
        const formData = new FormData();
        formData.append('db_path', dbPath);

        const response = await fetch('/api/contexts', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to list contexts');
        }

        return response.json();
    }

    /**
     * Load state tree for a specific context
     * @param {string} dbPath - Path to the database directory
     * @param {string} contextId - Context ID to load tree for
     * @param {File} wasmFile - WASM file (required for state tree)
     * @returns {Promise<{data: Object}>}
     * @throws {Error} If the request fails or WASM file is missing
     */
    static async loadContextTree(dbPath, contextId, wasmFile) {
        const formData = new FormData();
        formData.append('db_path', dbPath);
        formData.append('context_id', contextId);
        formData.append('wasm_file', wasmFile);

        const response = await fetch('/api/context-tree', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to load context tree');
        }

        return response.json();
    }

    /**
     * Load state tree visualization data (legacy - loads all contexts)
     * @deprecated Use listContexts() and loadContextTree() for better performance
     * @param {string} dbPath - Path to the database directory
     * @param {File} wasmFile - WASM file (required for state tree)
     * @returns {Promise<{data: Object}>}
     * @throws {Error} If the request fails or WASM file is missing
     */
    static async loadStateTree(dbPath, wasmFile) {
        const formData = new FormData();
        formData.append('db_path', dbPath);
        formData.append('wasm_file', wasmFile);

        const response = await fetch('/api/state-tree', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to load state tree');
        }

        return response.json();
    }

    /**
     * Load DAG visualization data
     * @param {string} dbPath - Path to the database directory
     * @returns {Promise<Object>}
     * @throws {Error} If the request fails
     */
    static async loadDAG(dbPath) {
        const formData = new FormData();
        formData.append('db_path', dbPath);

        const response = await fetch('/api/dag', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to load DAG');
        }

        return response.json();
    }

    /**
     * Validate that a WASM file contains an ABI
     * @param {File} wasmFile - WASM file to validate
     * @returns {Promise<{data: {has_abi: boolean}, warning?: string, info?: string}>}
     * @throws {Error} If the request fails
     */
    static async validateAbi(wasmFile) {
        const formData = new FormData();
        formData.append('wasm_file', wasmFile);

        const response = await fetch('/api/validate-abi', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to validate ABI');
        }

        return response.json();
    }

    /**
     * Load detailed information about a specific delta (on-demand for tooltips)
     * @param {string} dbPath - Path to the database directory
     * @param {string} contextId - Context ID
     * @param {string} deltaId - Delta ID
     * @returns {Promise<{context_id: string, delta_id: string, actions?: Array, events?: Array}>}
     * @throws {Error} If the request fails
     */
    static async loadDeltaDetails(dbPath, contextId, deltaId) {
        const formData = new FormData();
        formData.append('db_path', dbPath);
        formData.append('context_id', contextId);
        formData.append('delta_id', deltaId);

        const response = await fetch('/api/dag/delta-details', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to load delta details');
        }

        return response.json();
    }
}
