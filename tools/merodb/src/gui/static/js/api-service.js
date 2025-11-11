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
     * Load state tree visualization data
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
}
