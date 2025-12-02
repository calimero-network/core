/**
 * Application State Manager
 * Centralized state management for the application
 * @module app-state
 */

export class AppState {
    constructor() {
        /** @type {Object|null} JSON data from database */
        this.jsonData = null;

        /** @type {string|null} Current database path */
        this.currentDbPath = null;

        /** @type {File|null} Current state schema file */
        this.currentStateSchemaFile = null;
        
        /** @type {string|null} Cached state schema file content (File objects can only be read once) */
        this.currentStateSchemaFileContent = null;

        /** @type {string|null} State schema file name (for display) */
        this.currentStateSchemaFileName = null;

        /** @type {string[]} Query history (max 10 items) */
        this.queryHistory = [];

        /** @type {Object|null} JQ library promise interface */
        this.jqReady = window.jq ? window.jq.promised : null;

        /** @type {Object|null} Processed DAG data */
        this.dagData = null;

        /** @type {Object|null} State tree hierarchy data */
        this.stateTreeData = null;

        /** @type {string} Currently active tab */
        this.currentTab = 'data';

        // Load from local storage on initialization
        this.loadFromLocalStorage();
    }

    /**
     * Load state from local storage
     */
    loadFromLocalStorage() {
        try {
            const savedDbPath = localStorage.getItem('merodb_db_path');
            const savedSchemaContent = localStorage.getItem('merodb_schema_content');
            const savedSchemaName = localStorage.getItem('merodb_schema_name');

            if (savedDbPath) {
                this.currentDbPath = savedDbPath;
            }
            if (savedSchemaContent) {
                this.currentStateSchemaFileContent = savedSchemaContent;
            }
            if (savedSchemaName) {
                this.currentStateSchemaFileName = savedSchemaName;
            }

            if (savedDbPath || savedSchemaContent) {
                console.log('[AppState] Loaded from local storage:', {
                    hasDbPath: !!savedDbPath,
                    hasSchemaContent: !!savedSchemaContent,
                    schemaName: savedSchemaName
                });
            }
        } catch (err) {
            console.warn('[AppState] Failed to load from local storage:', err);
        }
    }

    /**
     * Save state to local storage
     */
    saveToLocalStorage() {
        try {
            if (this.currentDbPath) {
                localStorage.setItem('merodb_db_path', this.currentDbPath);
            } else {
                localStorage.removeItem('merodb_db_path');
            }

            if (this.currentStateSchemaFileContent) {
                localStorage.setItem('merodb_schema_content', this.currentStateSchemaFileContent);
            } else {
                localStorage.removeItem('merodb_schema_content');
            }

            if (this.currentStateSchemaFileName) {
                localStorage.setItem('merodb_schema_name', this.currentStateSchemaFileName);
            } else {
                localStorage.removeItem('merodb_schema_name');
            }

            console.log('[AppState] Saved to local storage');
        } catch (err) {
            console.warn('[AppState] Failed to save to local storage:', err);
        }
    }

    /**
     * Clear local storage
     */
    clearLocalStorage() {
        try {
            localStorage.removeItem('merodb_db_path');
            localStorage.removeItem('merodb_schema_content');
            localStorage.removeItem('merodb_schema_name');
            console.log('[AppState] Cleared local storage');
        } catch (err) {
            console.warn('[AppState] Failed to clear local storage:', err);
        }
    }

    /**
     * Set loaded data and metadata
     * @param {Object} data - JSON data from database
     * @param {string} dbPath - Database path
     * @param {File|null} stateSchemaFile - State schema file
     */
    setData(data, dbPath, stateSchemaFile) {
        this.jsonData = data;
        this.currentDbPath = dbPath;
        
        console.log('[AppState.setData] Called with:', {
            dbPath,
            stateSchemaFile: stateSchemaFile ? {
                name: stateSchemaFile.name,
                type: stateSchemaFile.type,
                size: stateSchemaFile.size,
                isSameAsCurrent: stateSchemaFile === this.currentStateSchemaFile
            } : null
        });
        
        // Store state schema file
        this.currentStateSchemaFile = stateSchemaFile || null;
        
        // If we have a state schema file but no cached content, try to cache it
        // (but only if it hasn't been consumed yet - this is a best-effort attempt)
        if (this.currentStateSchemaFile && !this.currentStateSchemaFileContent) {
            // Try to read and cache, but don't fail if it's already consumed
            this.currentStateSchemaFile.text().then(text => {
                this.currentStateSchemaFileContent = text;
                this.currentStateSchemaFileName = this.currentStateSchemaFile.name;
                this.saveToLocalStorage();
                console.log('[AppState.setData] Cached state schema file content, length:', text.length);
            }).catch(err => {
                console.warn('[AppState.setData] Could not cache state schema file (may already be consumed):', err.message);
                // Don't set to null - keep existing cache if any
                // Try to restore from local storage
                try {
                    const savedContent = localStorage.getItem('merodb_schema_content');
                    if (savedContent) {
                        this.currentStateSchemaFileContent = savedContent;
                        console.log('[AppState.setData] Restored schema content from local storage');
                    }
                } catch (storageErr) {
                    console.warn('[AppState.setData] Failed to restore from local storage:', storageErr);
                }
            });
        } else if (!this.currentStateSchemaFile) {
            // Don't clear cached content if we're auto-loading - preserve it from local storage
            // Only clear if we explicitly don't want it
            if (!this.currentStateSchemaFileContent) {
                // Try to restore from local storage one more time
                try {
                    const savedContent = localStorage.getItem('merodb_schema_content');
                    if (savedContent) {
                        this.currentStateSchemaFileContent = savedContent;
                        const savedName = localStorage.getItem('merodb_schema_name');
                        if (savedName) {
                            this.currentStateSchemaFileName = savedName;
                        }
                        console.log('[AppState.setData] Restored schema content from local storage (no file object)');
                    }
                } catch (storageErr) {
                    console.warn('[AppState.setData] Failed to restore from local storage:', storageErr);
                }
            }
        }
        
        console.log('✅ State schema file set:', {
            stateSchemaFile: this.currentStateSchemaFile?.name || 'null',
            hasCachedContent: !!this.currentStateSchemaFileContent
        });

        // Save to local storage
        this.saveToLocalStorage();

        // Reset visualization data when loading new database
        this.dagData = null;
        this.stateTreeData = null;
    }

    /**
     * Add query to history (deduplicates and limits to 10)
     * @param {string} query - JQ query string
     */
    addToHistory(query) {
        // Remove if already exists
        const index = this.queryHistory.indexOf(query);
        if (index !== -1) {
            this.queryHistory.splice(index, 1);
        }

        // Add to beginning
        this.queryHistory.unshift(query);

        // Keep only last 10
        if (this.queryHistory.length > 10) {
            this.queryHistory = this.queryHistory.slice(0, 10);
        }
    }

    /**
     * Clear all state (useful for reload)
     */
    reset() {
        this.dagData = null;
        this.stateTreeData = null;
    }

    /**
     * Reset to initial state (clear everything including local storage)
     */
    resetAll() {
        this.jsonData = null;
        this.currentDbPath = null;
        this.currentStateSchemaFile = null;
        this.currentStateSchemaFileContent = null;
        this.currentStateSchemaFileName = null;
        this.dagData = null;
        this.stateTreeData = null;
        this.clearLocalStorage();
    }
}
