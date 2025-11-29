/**
 * API Service Module
 * Handles all HTTP communication with the backend server
 * @module api-service
 */

export class ApiService {
    /**
     * Load database and optionally extract schema from state schema file
     * @param {string} dbPath - Path to the database directory
     * @param {File|null} stateSchemaFile - Optional state schema JSON file
     * @returns {Promise<{data: Object, warning?: string, info?: string}>}
     * @throws {Error} If the request fails or database doesn't exist
     */
    static async loadDatabase(dbPath, stateSchemaFile) {
        console.log('[ApiService.loadDatabase] Called with:', {
            dbPath,
            stateSchemaFile: stateSchemaFile ? {
                name: stateSchemaFile.name,
                size: stateSchemaFile.size,
                type: stateSchemaFile.type
            } : null
        });
        
        const formData = new FormData();
        formData.append('db_path', dbPath);

        // Try to get cached content first (File objects can only be read once)
        // Also check local storage as a fallback
        let text;
        if (window.app?.state?.currentStateSchemaFileContent) {
            console.log('[ApiService.loadDatabase] Using cached state schema content from state, length:', window.app.state.currentStateSchemaFileContent.length);
            text = window.app.state.currentStateSchemaFileContent;
        } else {
            // Try local storage as fallback
            try {
                const savedContent = localStorage.getItem('merodb_schema_content');
                if (savedContent) {
                    console.log('[ApiService.loadDatabase] Using cached state schema content from local storage, length:', savedContent.length);
                    text = savedContent;
                    // Restore to state for future use
                    if (window.app?.state) {
                        window.app.state.currentStateSchemaFileContent = savedContent;
                    }
                } else if (stateSchemaFile) {
                    console.log('[ApiService.loadDatabase] Reading state schema file (not cached):', stateSchemaFile.name);
                    text = await stateSchemaFile.text();
                    // Cache it for future use
                    if (window.app?.state) {
                        window.app.state.currentStateSchemaFileContent = text;
                        window.app.state.saveToLocalStorage();
                    }
                } else {
                    console.log('[ApiService.loadDatabase] No state schema file or cached content available');
                }
            } catch (err) {
                console.warn('[ApiService.loadDatabase] Error accessing local storage:', err);
            }
        }
        
        if (text) {
            console.log('[ApiService.loadDatabase] State schema file content length:', text.length, 'First 100 chars:', text.substring(0, 100));
            formData.append('state_schema_file', text);
        } else {
            console.log('[ApiService.loadDatabase] No state schema file or cached content provided');
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
     * @param {File|null} stateSchemaFile - Optional state schema file
     * @returns {Promise<{data: Object}>}
     * @throws {Error} If the request fails or state schema file is missing
     */
    static async loadContextTree(dbPath, contextId, stateSchemaFile) {
        console.log('[ApiService.loadContextTree] Called with:', {
            dbPath,
            contextId,
            stateSchemaFile: stateSchemaFile ? {
                name: stateSchemaFile.name,
                size: stateSchemaFile.size,
                type: stateSchemaFile.type
            } : null
        });
        
        const formData = new FormData();
        formData.append('db_path', dbPath);
        formData.append('context_id', contextId);

        // Try to get cached content first (File objects can only be read once)
        // Check cached content even if stateSchemaFile is null
        // Also check local storage as a fallback
        let text;
        if (window.app?.state?.currentStateSchemaFileContent) {
            console.log('[ApiService.loadContextTree] Using cached state schema content from state, length:', window.app.state.currentStateSchemaFileContent.length);
            text = window.app.state.currentStateSchemaFileContent;
        } else {
            // Try local storage as fallback
            try {
                const savedContent = localStorage.getItem('merodb_schema_content');
                if (savedContent) {
                    console.log('[ApiService.loadContextTree] Using cached state schema content from local storage, length:', savedContent.length);
                    text = savedContent;
                    // Restore to state for future use
                    if (window.app?.state) {
                        window.app.state.currentStateSchemaFileContent = savedContent;
                    }
                } else if (stateSchemaFile) {
                    console.log('[ApiService.loadContextTree] Reading state schema file (not cached):', stateSchemaFile.name);
                    try {
                        text = await stateSchemaFile.text();
                        // Cache it for future use
                        if (window.app?.state) {
                            window.app.state.currentStateSchemaFileContent = text;
                            window.app.state.currentStateSchemaFile = stateSchemaFile; // Update reference
                            window.app.state.saveToLocalStorage();
                            console.log('[ApiService.loadContextTree] Cached state schema content, length:', text.length);
                        }
                    } catch (err) {
                        console.error('[ApiService.loadContextTree] Failed to read state schema file:', err);
                        throw new Error(`Failed to read state schema file: ${err.message}. The file may have already been consumed.`);
                    }
                } else {
                    console.error('[ApiService.loadContextTree] ERROR: No state schema file or cached content available!');
                    console.error('[ApiService.loadContextTree] State:', {
                        currentStateSchemaFile: window.app?.state?.currentStateSchemaFile?.name || 'null',
                        hasCachedContent: !!window.app?.state?.currentStateSchemaFileContent,
                        hasLocalStorageContent: !!localStorage.getItem('merodb_schema_content'),
                        stateSchemaFileProvided: !!stateSchemaFile
                    });
                    throw new Error('State schema file is required for state tree extraction');
                }
            } catch (err) {
                if (err.message.includes('State schema file is required')) {
                    throw err;
                }
                console.error('[ApiService.loadContextTree] Error accessing local storage:', err);
                throw new Error('State schema file is required for state tree extraction');
            }
        }
        
        console.log('[ApiService.loadContextTree] Appending state_schema_file to formData, length:', text.length);
        formData.append('state_schema_file', text);

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
     * @param {File|null} stateSchemaFile - Optional state schema file
     * @returns {Promise<{data: Object}>}
     * @throws {Error} If the request fails
     */
    static async loadStateTree(dbPath, stateSchemaFile) {
        const formData = new FormData();
        formData.append('db_path', dbPath);

        if (stateSchemaFile) {
            const text = await stateSchemaFile.text();
            formData.append('state_schema_file', text);
        }

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
     * Validate that a state schema file is valid
     * @param {File} stateSchemaFile - State schema file to validate
     * @returns {Promise<{data: {valid: boolean}, warning?: string, info?: string}>}
     * @throws {Error} If the request fails
     */
    static async validateStateSchema(stateSchemaFile) {
        const formData = new FormData();
        const text = await stateSchemaFile.text();
        formData.append('state_schema_file', text);

        const response = await fetch('/api/validate-abi', {
            method: 'POST',
            body: formData
        });

        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to validate state schema');
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
