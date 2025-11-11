/**
 * JQ Query Manager Module
 * Handles JQ query execution, examples, and history
 * @module jq-manager
 */

import { UIManager } from './ui-manager.js';

export class JQManager {
    /**
     * @param {import('./app-state.js').AppState} state - Application state
     */
    constructor(state) {
        this.state = state;

        // Predefined example queries
        this.examples = [
            '.',
            '. | keys',
            '.Meta',
            '.Delta',
            '.Meta | keys',
            '.Delta | to_entries | .[0]',
            '.Delta | to_entries | map({context: .key, count: (.value | length)})',
            '.Meta.contexts | length'
        ];

        this.init();
    }

    /**
     * Initialize the JQ panel - render examples and bind events
     */
    init() {
        this.renderExamples();
        this.bindEvents();
    }

    /**
     * Render example query buttons
     */
    renderExamples() {
        const container = document.getElementById('examples');
        if (!container) return;

        container.innerHTML = '';

        this.examples.forEach(example => {
            const btn = UIManager.createElement('button', 'jq-panel__example-button');
            btn.textContent = example;
            btn.addEventListener('click', () => this.loadExample(example));
            container.appendChild(btn);
        });
    }

    /**
     * Bind event listeners
     */
    bindEvents() {
        // Run button
        const runBtn = document.getElementById('run-query');
        if (runBtn) {
            runBtn.addEventListener('click', () => this.runQuery());
        }

        // Enter key in input
        const input = document.getElementById('jq-query');
        if (input) {
            input.addEventListener('keydown', (e) => {
                if (e.key === 'Enter') {
                    this.runQuery();
                }
            });
        }
    }

    /**
     * Load an example query into the input
     * @param {string} query - Query string
     */
    loadExample(query) {
        const input = document.getElementById('jq-query');
        if (input) {
            input.value = query;
            input.focus();
        }
    }

    /**
     * Execute the current query
     */
    async runQuery() {
        const input = document.getElementById('jq-query');
        if (!input) return;

        const query = input.value.trim();
        if (!query || !this.state.jsonData) {
            this.showError('No query or data available');
            return;
        }

        try {
            // Execute query using jq-web library
            const result = await this.state.jqReady.json(this.state.jsonData, query);

            // Show result
            this.showSuccess(JSON.stringify(result, null, 2));

            // Add to history
            this.state.addToHistory(query);
            this.updateHistory();
        } catch (error) {
            this.showError(error.message || 'Query execution failed');
        }
    }

    /**
     * Display successful query result
     * @param {string} result - Formatted JSON result
     */
    showSuccess(result) {
        const container = document.getElementById('result');
        if (!container) return;

        container.innerHTML = `
            <pre class="jq-panel__result-success">
                <code>${UIManager.escapeHtml(result)}</code>
            </pre>
        `;
    }

    /**
     * Display query error
     * @param {string} message - Error message
     */
    showError(message) {
        const container = document.getElementById('result');
        if (!container) return;

        container.innerHTML = `
            <div class="jq-panel__result-error">
                <strong>Error:</strong> ${UIManager.escapeHtml(message)}
            </div>
        `;
    }

    /**
     * Update history button list
     */
    updateHistory() {
        const container = document.getElementById('history');
        if (!container) return;

        container.innerHTML = '';

        this.state.queryHistory.forEach(query => {
            const btn = UIManager.createElement('button', 'jq-panel__history-button');
            btn.textContent = query;
            btn.addEventListener('click', () => this.loadExample(query));
            container.appendChild(btn);
        });
    }
}
