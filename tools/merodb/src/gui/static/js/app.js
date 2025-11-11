/**
 * Main Application Controller
 * Coordinates all modules and handles user interactions
 * @module app
 */

import { AppState } from './app-state.js';
import { ApiService } from './api-service.js';
import { UIManager } from './ui-manager.js';
import { JSONRenderer } from './json-renderer.js';
import { JQManager } from './jq-manager.js';
import { DAGVisualizer } from './dag-visualizer.js';
import { StateTreeVisualizer } from './state-tree-visualizer.js';

export class App {
    constructor() {
        this.state = new AppState();
        this.jqManager = null;
        this.dagVisualizer = null;
        this.stateVisualizer = null;

        this.init();
    }

    /**
     * Initialize application - bind all event handlers
     */
    init() {
        this.setupFormHandlers();
        this.setupTabHandlers();
        this.setupControlHandlers();
    }

    /**
     * Setup form-related event handlers
     */
    setupFormHandlers() {
        // WASM file selection
        const wasmInput = document.getElementById('wasm-input');
        if (wasmInput) {
            wasmInput.addEventListener('change', async (e) => {
                const file = e.target.files[0];
                const filename = document.getElementById('wasm-filename');
                if (filename) {
                    filename.textContent = file ? file.name : 'No file chosen';
                }

                // Validate ABI immediately when file is selected
                if (file) {
                    await this.validateWasmAbi(file);
                }
            });
        }

        // Form submission
        const loadForm = document.getElementById('load-form');
        if (loadForm) {
            loadForm.addEventListener('submit', async (e) => {
                e.preventDefault();
                await this.loadDatabase();
            });
        }

        // Reload button
        const reloadBtn = document.getElementById('reload-button');
        if (reloadBtn) {
            reloadBtn.addEventListener('click', async () => {
                await this.loadDatabase();
            });
        }
    }

    /**
     * Setup tab navigation handlers
     */
    setupTabHandlers() {
        document.querySelectorAll('.tabs__button').forEach(btn => {
            btn.addEventListener('click', async () => {
                const tab = btn.dataset.tab;
                if (tab) {
                    await this.switchTab(tab);
                }
            });
        });
    }

    /**
     * Setup visualization control handlers
     */
    setupControlHandlers() {
        // DAG controls
        const layoutSelect = document.getElementById('layout-select');
        if (layoutSelect) {
            layoutSelect.addEventListener('change', () => {
                if (this.dagVisualizer) {
                    this.dagVisualizer.render();
                }
            });
        }

        const resetZoomBtn = document.getElementById('reset-zoom');
        if (resetZoomBtn) {
            resetZoomBtn.addEventListener('click', () => {
                if (this.dagVisualizer) {
                    this.dagVisualizer.resetZoom();
                }
            });
        }

        const exportDagBtn = document.getElementById('export-dag');
        if (exportDagBtn) {
            exportDagBtn.addEventListener('click', () => {
                if (this.dagVisualizer) {
                    this.dagVisualizer.exportImage();
                }
            });
        }

        // State tree controls
        const stateLayoutSelect = document.getElementById('state-layout-select');
        if (stateLayoutSelect) {
            stateLayoutSelect.addEventListener('change', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.render();
                }
            });
        }

        const stateContextSelect = document.getElementById('state-context-select');
        if (stateContextSelect) {
            stateContextSelect.addEventListener('change', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.render();
                }
            });
        }

        const resetStateZoomBtn = document.getElementById('reset-state-zoom');
        if (resetStateZoomBtn) {
            resetStateZoomBtn.addEventListener('click', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.resetZoom();
                }
            });
        }

        const expandAllBtn = document.getElementById('expand-all');
        if (expandAllBtn) {
            expandAllBtn.addEventListener('click', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.expandAll();
                }
            });
        }

        const collapseAllBtn = document.getElementById('collapse-all');
        if (collapseAllBtn) {
            collapseAllBtn.addEventListener('click', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.collapseAll();
                }
            });
        }

        const exportStateBtn = document.getElementById('export-state');
        if (exportStateBtn) {
            exportStateBtn.addEventListener('click', () => {
                if (this.stateVisualizer) {
                    this.stateVisualizer.exportImage();
                }
            });
        }
    }

    /**
     * Validate WASM file for ABI presence
     * @param {File} wasmFile - WASM file to validate
     */
    async validateWasmAbi(wasmFile) {
        UIManager.hideMessage('warning-message');
        UIManager.hideMessage('info-message');

        try {
            const response = await ApiService.validateAbi(wasmFile);

            if (response.warning) {
                UIManager.showMessage('warning-message', response.warning);
            }
            if (response.info) {
                UIManager.showMessage('info-message', response.info);
            }
        } catch (error) {
            UIManager.showMessage('warning-message', `WASM validation warning: ${error.message}`);
        }
    }

    /**
     * Load database and display data
     */
    async loadDatabase() {
        const dbPathInput = document.getElementById('db-path-input');
        const wasmInput = document.getElementById('wasm-input');
        const loadButton = document.getElementById('load-button');

        if (!dbPathInput || !loadButton) return;

        const dbPath = dbPathInput.value.trim();
        const wasmFile = wasmInput?.files[0] || null;

        if (!dbPath) {
            UIManager.showMessage('error-message', 'Database path is required');
            return;
        }

        // Hide all messages
        UIManager.hideMessage('error-message');
        UIManager.hideMessage('warning-message');
        UIManager.hideMessage('info-message');

        // Show loading state
        UIManager.showElement('loading-message');
        loadButton.disabled = true;

        try {
            // Load database via API
            const response = await ApiService.loadDatabase(dbPath, wasmFile);

            // Store in state
            this.state.setData(response.data, dbPath, wasmFile);

            // Show messages if any
            if (response.warning) {
                UIManager.showMessage('warning-message', response.warning);
            }
            if (response.info) {
                UIManager.showMessage('info-message', response.info);
            }

            // Transition to viewer after short delay
            setTimeout(() => {
                this.showViewer();
            }, 500);

        } catch (error) {
            UIManager.showMessage('error-message', error.message);
        } finally {
            UIManager.hideElement('loading-message');
            loadButton.disabled = false;
        }
    }

    /**
     * Show viewer UI and initialize components
     */
    showViewer() {
        UIManager.showViewer();

        // Update file info
        const dbPathDisplay = document.getElementById('current-db-path');
        if (dbPathDisplay) {
            dbPathDisplay.textContent = this.state.currentDbPath;
        }

        // Initialize all managers
        JSONRenderer.render(this.state.jsonData, 'json-viewer');
        this.jqManager = new JQManager(this.state);
        this.dagVisualizer = new DAGVisualizer(this.state);
        this.stateVisualizer = new StateTreeVisualizer(this.state);
    }

    /**
     * Switch between tabs (data, dag, state)
     * @param {string} tab - Tab name to switch to
     */
    async switchTab(tab) {
        // Update active tab button
        UIManager.setActiveTab(tab);

        // Hide all views
        UIManager.hideElement('data-view');
        UIManager.hideElement('dag-view');
        UIManager.hideElement('state-view');

        // Update state
        this.state.currentTab = tab;

        // Show selected view
        if (tab === 'data') {
            UIManager.showElement('data-view');
        } else if (tab === 'dag') {
            UIManager.showElement('dag-view');
            if (!this.state.dagData) {
                try {
                    await this.dagVisualizer.load();
                } catch (error) {
                    UIManager.showMessage('error-message', error.message);
                }
            }
        } else if (tab === 'state') {
            UIManager.showElement('state-view');
            if (!this.state.stateTreeData) {
                try {
                    await this.stateVisualizer.load();
                } catch (error) {
                    const errorEl = document.getElementById('state-error');
                    if (errorEl) {
                        errorEl.textContent = error.message;
                        UIManager.showElement('state-error');
                    }
                }
            }
        }
    }
}

// Initialize application when DOM is ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => {
        new App();
    });
} else {
    new App();
}
