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
        this.loadFromLocalStorage();
    }

    /**
     * Load saved state from local storage and populate form
     */
    async loadFromLocalStorage() {
        // Always try to load from local storage first (in case state wasn't initialized yet)
        if (!this.state.currentDbPath || !this.state.currentStateSchemaFileContent) {
            this.state.loadFromLocalStorage();
        }

        // Populate form fields
        if (this.state.currentDbPath) {
            const dbPathInput = document.getElementById('db-path-input');
            if (dbPathInput) {
                dbPathInput.value = this.state.currentDbPath;
                console.log('[App.loadFromLocalStorage] Restored db path:', this.state.currentDbPath);
            }
        }

        if (this.state.currentStateSchemaFileName) {
            const filename = document.getElementById('state-schema-filename');
            if (filename) {
                filename.textContent = this.state.currentStateSchemaFileName;
                console.log('[App.loadFromLocalStorage] Restored schema filename:', this.state.currentStateSchemaFileName);
            }
        }

        // If we have both database path and schema content, automatically load the database
        // Wait a bit to ensure DOM is fully ready
        if (this.state.currentDbPath && this.state.currentStateSchemaFileContent) {
            console.log('[App] Auto-loading database from local storage:', {
                dbPath: this.state.currentDbPath,
                hasSchemaContent: !!this.state.currentStateSchemaFileContent,
                schemaName: this.state.currentStateSchemaFileName
            });
            
            // Small delay to ensure all DOM elements are ready
            setTimeout(async () => {
                try {
                    await this.loadDatabase();
                } catch (error) {
                    console.error('[App] Failed to auto-load database:', error);
                    UIManager.showMessage('warning-message', `Failed to auto-load: ${error.message}. Please load manually.`);
                }
            }, 100);
        } else {
            console.log('[App.loadFromLocalStorage] Not auto-loading - missing data:', {
                hasDbPath: !!this.state.currentDbPath,
                hasSchemaContent: !!this.state.currentStateSchemaFileContent
            });
        }
    }

    /**
     * Setup form-related event handlers
     */
    setupFormHandlers() {
        // State schema file selection
        const stateSchemaInput = document.getElementById('state-schema-input');
        if (stateSchemaInput) {
            stateSchemaInput.addEventListener('change', async (e) => {
                const file = e.target.files[0];
                const filename = document.getElementById('state-schema-filename');
                
                if (file) {
                    if (!file.name.endsWith('.json')) {
                        UIManager.showMessage('warning-message', 'State schema file should be a .json file.');
                        stateSchemaInput.value = '';
                        if (filename) {
                            filename.textContent = 'No file chosen';
                        }
                        // Clear state
                        this.state.currentStateSchemaFile = null;
                        this.updateSchemaInfo();
                        return;
                    }
                    
                    // Store in state immediately
                    this.state.currentStateSchemaFile = file;
                    
                    // CRITICAL: Cache the file content immediately (File objects can only be read once)
                    file.text().then(text => {
                        this.state.currentStateSchemaFileContent = text;
                        this.state.currentStateSchemaFileName = file.name;
                        this.state.saveToLocalStorage();
                        console.log('✅ Cached state schema file content immediately, length:', text.length);
                    }).catch(err => {
                        console.error('Failed to cache state schema file content:', err);
                    });
                    
                    console.log('✅ State schema file selected and stored:', file.name);
                } else {
                    // Clear state if no file
                    this.state.currentStateSchemaFile = null;
                }
                
                if (filename) {
                    filename.textContent = file ? file.name : 'No file chosen';
                }
                
                // Update schema info display
                this.updateSchemaInfo();
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

        // State schema file selector in viewer
        const viewerStateSchemaInput = document.getElementById('viewer-state-schema-input');
        if (viewerStateSchemaInput) {
            viewerStateSchemaInput.addEventListener('change', async (e) => {
                const file = e.target.files[0];
                if (file) {
                    if (!file.name.endsWith('.json')) {
                        UIManager.showMessage('error-message', 'State schema file should be a .json file.');
                        viewerStateSchemaInput.value = '';
                        return;
                    }
                    
                    console.log('State schema file selected from viewer:', file.name);
                    // Update state immediately
                    this.state.currentStateSchemaFile = file;
                    
                    // CRITICAL: Cache the file content immediately (File objects can only be read once)
                    file.text().then(text => {
                        this.state.currentStateSchemaFileContent = text;
                        this.state.currentStateSchemaFileName = file.name;
                        this.state.saveToLocalStorage();
                        console.log('✅ Cached state schema file content from viewer, length:', text.length);
                    }).catch(err => {
                        console.error('Failed to cache state schema file content from viewer:', err);
                    });
                    
                    this.updateSchemaInfo();
                    
                    // If state tree is the active tab, try to reload it
                    if (this.state.currentTab === 'state' && this.stateVisualizer) {
                        try {
                            await this.stateVisualizer.load();
                            this.stateVisualizer.render();
                        } catch (error) {
                            console.error('Failed to reload state tree:', error);
                            UIManager.showMessage('error-message', `Failed to reload state tree: ${error.message}`);
                        }
                    }
                } else {
                    // Clear state if no file selected
                    this.state.currentStateSchemaFile = null;
                    this.updateSchemaInfo();
                }
            });
        }

        // Back to main button
        const backButton = document.getElementById('back-button');
        if (backButton) {
            backButton.addEventListener('click', () => {
                this.goBackToMain();
            });
        }

        // Reload button
        const reloadBtn = document.getElementById('reload-button');
        if (reloadBtn) {
            reloadBtn.addEventListener('click', async () => {
                // Check if we have file in state, otherwise check form input
                if (!this.state.currentStateSchemaFile) {
                    const stateSchemaInput = document.getElementById('state-schema-input');
                    
                    // Try to restore from form input if available
                    if (stateSchemaInput?.files[0]) {
                        this.state.currentStateSchemaFile = stateSchemaInput.files[0];
                    }
                    
                    if (!this.state.currentStateSchemaFile) {
                        UIManager.showMessage('warning-message', 'No state schema file found. Please select a file first.');
                        return;
                    }
                }
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
        // Back to main button
        const backButton = document.getElementById('back-button');
        if (backButton) {
            backButton.addEventListener('click', () => {
                this.goBackToMain();
            });
        }

        // DAG controls
        const contextSelect = document.getElementById('context-select');
        if (contextSelect) {
            contextSelect.addEventListener('change', () => {
                if (this.dagVisualizer) {
                    this.dagVisualizer.render();
                }
            });
        }

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
     * Load database and display data
     */
    async loadDatabase() {
        const dbPathInput = document.getElementById('db-path-input');
        const stateSchemaInput = document.getElementById('state-schema-input');
        const loadButton = document.getElementById('load-button');

        if (!dbPathInput || !loadButton) {
            console.error('Missing required form elements');
            return;
        }

        const dbPath = dbPathInput.value.trim() || this.state.currentDbPath;
        
        // Check state first (for files selected via viewer buttons), then fall back to form input
        // Also check if we have cached content from local storage (for auto-load)
        let stateSchemaFile = this.state.currentStateSchemaFile || stateSchemaInput?.files[0] || null;
        
        // If we don't have a file object but we have cached content, that's okay - the API will use cached content
        if (!stateSchemaFile && this.state.currentStateSchemaFileContent) {
            console.log('[loadDatabase] No file object, but have cached schema content from local storage');
        }

        console.log('Loading database with file:', {
            dbPath,
            fromState: {
                stateSchemaFile: this.state.currentStateSchemaFile?.name || 'none',
                hasCachedContent: !!this.state.currentStateSchemaFileContent
            },
            fromForm: {
                stateSchemaFile: stateSchemaInput?.files[0]?.name || 'none'
            },
            final: {
                stateSchemaFile: stateSchemaFile?.name || 'none',
                hasCachedContent: !!this.state.currentStateSchemaFileContent
            }
        });

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
            // Debug: Log file selection
            console.log('Loading database with:', {
                dbPath,
                stateSchemaFile: stateSchemaFile?.name || 'none'
            });
            
            // Load database via API
            console.log('[loadDatabase] Calling ApiService.loadDatabase with:', {
                stateSchemaFile: stateSchemaFile?.name || 'null'
            });
            const response = await ApiService.loadDatabase(dbPath, stateSchemaFile);

            // Store in state - response.data contains {data: {...}, database: "...", exported_columns: [...]}
            // We need to extract the actual data object
            const actualData = response.data?.data || response.data;
            
            // Debug: Log the data structure
            console.log('API Response structure:', {
                hasData: !!response.data,
                hasNestedData: !!response.data?.data,
                actualDataKeys: actualData ? Object.keys(actualData) : null,
                hasState: actualData?.State !== undefined,
                stateCount: actualData?.State?.count,
                stateEntries: actualData?.State?.entries?.length
            });
            
            // Set data in state
            // If we don't have a file object but have cached content, preserve it
            const schemaFileToSet = stateSchemaFile || (this.state.currentStateSchemaFileContent ? this.state.currentStateSchemaFile : null);
            this.state.setData(actualData, dbPath, schemaFileToSet);
            
            // Ensure schema content is preserved even if file object is null
            if (!this.state.currentStateSchemaFileContent && stateSchemaFile) {
                // This shouldn't happen, but just in case
                console.warn('[loadDatabase] Schema content was lost, trying to restore from local storage');
                try {
                    const savedContent = localStorage.getItem('merodb_schema_content');
                    if (savedContent) {
                        this.state.currentStateSchemaFileContent = savedContent;
                    }
                } catch (err) {
                    console.error('[loadDatabase] Failed to restore schema content:', err);
                }
            }
            
            // Debug: Verify state was set
            console.log('State after loading:', {
                stateSchemaFile: this.state.currentStateSchemaFile?.name || 'none',
                jsonDataIsNull: this.state.jsonData === null,
                jsonDataKeys: this.state.jsonData ? Object.keys(this.state.jsonData) : null
            });

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
     * Go back to main screen
     */
    goBackToMain() {
        UIManager.hideViewer();
        UIManager.showUpload();
        // Restore form fields from state/local storage
        this.loadFromLocalStorage();
        // Don't clear state - keep it for next time
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
        
        this.updateSchemaInfo();

        // Debug: Log what we're about to render
        console.log('showViewer - jsonData:', {
            isNull: this.state.jsonData === null,
            isUndefined: this.state.jsonData === undefined,
            type: typeof this.state.jsonData,
            keys: this.state.jsonData ? Object.keys(this.state.jsonData) : null,
            hasState: this.state.jsonData?.State !== undefined,
            stateCount: this.state.jsonData?.State?.count
        });

        // Initialize all managers
        if (this.state.jsonData) {
            JSONRenderer.render(this.state.jsonData, 'json-viewer');
        } else {
            console.error('No JSON data to render!');
            const container = document.getElementById('json-viewer');
            if (container) {
                container.innerHTML = '<div style="padding: 20px; color: #f44336;">No data loaded. Please reload the database.</div>';
            }
        }
        this.jqManager = new JQManager(this.state);
        this.dagVisualizer = new DAGVisualizer(this.state);
        this.stateVisualizer = new StateTreeVisualizer(this.state);
    }

    /**
     * Update the schema file info display
     */
    updateSchemaInfo() {
        const schemaInfoDisplay = document.getElementById('current-schema-info');
        if (schemaInfoDisplay) {
            if (this.state.currentStateSchemaFile) {
                schemaInfoDisplay.textContent = `📄 Schema: ${this.state.currentStateSchemaFile.name}`;
                schemaInfoDisplay.style.color = '#4CAF50';
            } else {
                schemaInfoDisplay.textContent = '⚠️ No schema file';
                schemaInfoDisplay.style.color = '#f44336';
            }
        }
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
let appInstance;
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => {
        appInstance = new App();
        window.app = appInstance; // Expose for debugging
    });
} else {
    appInstance = new App();
    window.app = appInstance; // Expose for debugging
}
