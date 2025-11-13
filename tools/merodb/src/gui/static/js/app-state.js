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

        /** @type {File|null} Current WASM file */
        this.currentWasmFile = null;

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
    }

    /**
     * Set loaded data and metadata
     * @param {Object} data - JSON data from database
     * @param {string} dbPath - Database path
     * @param {File|null} wasmFile - WASM file
     */
    setData(data, dbPath, wasmFile) {
        this.jsonData = data;
        this.currentDbPath = dbPath;
        this.currentWasmFile = wasmFile;

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
}
