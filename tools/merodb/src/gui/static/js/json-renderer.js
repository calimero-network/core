/**
 * JSON Renderer Module
 * Displays JSON data with syntax highlighting and collapsible nodes
 * @module json-renderer
 */

import { UIManager } from './ui-manager.js';

export class JSONRenderer {
    /**
     * Render JSON data into a container
     * @param {*} data - JSON data to render
     * @param {string} containerId - Container element ID
     * @param {boolean} collapseChildren - Whether to collapse child nodes (default: true)
     */
    static render(data, containerId, collapseChildren = true) {
        const container = document.getElementById(containerId);
        if (!container) {
            console.error(`Container ${containerId} not found`);
            return;
        }

        container.innerHTML = '';
        // Render root with expanded state, but collapse its children
        container.appendChild(this.createNode(data, null, false, collapseChildren));
    }

    /**
     * Create a DOM node for a JSON value
     * @param {*} value - JSON value
     * @param {string|null} key - Object key or array index
     * @param {boolean} startCollapsed - Whether to start collapsed
     * @param {boolean} collapseChildren - Whether children should start collapsed
     * @returns {HTMLElement}
     */
    static createNode(value, key = null, startCollapsed = false, collapseChildren = false) {
        const div = document.createElement('div');
        div.className = 'json-viewer__node';

        if (Array.isArray(value)) {
            this.renderArray(div, value, key, startCollapsed, collapseChildren);
        } else if (value !== null && typeof value === 'object') {
            this.renderObject(div, value, key, startCollapsed, collapseChildren);
        } else {
            this.renderPrimitive(div, value, key);
        }

        return div;
    }

    /**
     * Render an object with collapsible behavior
     * @param {HTMLElement} container - Container element
     * @param {Object} obj - Object to render
     * @param {string|null} key - Object key
     * @param {boolean} startCollapsed - Whether to start collapsed
     * @param {boolean} collapseChildren - Whether children should start collapsed
     */
    static renderObject(container, obj, key, startCollapsed = false, collapseChildren = false) {
        const entries = Object.entries(obj);
        const keySpan = key
            ? `<span class="json-viewer__key">"${UIManager.escapeHtml(key)}"</span><span class="json-viewer__colon">: </span>`
            : '';

        // Header with expand/collapse
        const header = document.createElement('div');
        header.className = 'json-viewer__key-container';
        const expandIcon = startCollapsed ? '▶' : '▼';
        header.innerHTML = `<span class="json-viewer__expand-icon">${expandIcon}</span>${keySpan}<span class="json-viewer__brace">{</span><span class="json-viewer__count"> ${entries.length} items</span>`;

        // Children container
        const childrenContainer = document.createElement('div');
        childrenContainer.className = 'json-viewer__children';
        if (startCollapsed) {
            childrenContainer.style.display = 'none';
        }

        entries.forEach(([k, v]) => {
            const entry = document.createElement('div');
            entry.className = 'json-viewer__entry';
            // Children inherit the collapseChildren flag as their startCollapsed state
            entry.appendChild(this.createNode(v, k, collapseChildren, false));
            childrenContainer.appendChild(entry);
        });

        // Footer
        const footer = document.createElement('div');
        footer.innerHTML = '<span class="json-viewer__brace">}</span>';
        footer.style.marginLeft = '20px';
        if (startCollapsed) {
            footer.style.display = 'none';
        }

        // Toggle functionality
        header.addEventListener('click', () => {
            const isExpanded = childrenContainer.style.display !== 'none';
            childrenContainer.style.display = isExpanded ? 'none' : 'block';
            footer.style.display = isExpanded ? 'none' : 'block';
            header.querySelector('.json-viewer__expand-icon').textContent = isExpanded ? '▶' : '▼';
        });

        container.appendChild(header);
        container.appendChild(childrenContainer);
        container.appendChild(footer);
    }

    /**
     * Render an array with collapsible behavior
     * @param {HTMLElement} container - Container element
     * @param {Array} arr - Array to render
     * @param {string|null} key - Object key
     * @param {boolean} startCollapsed - Whether to start collapsed
     * @param {boolean} collapseChildren - Whether children should start collapsed
     */
    static renderArray(container, arr, key, startCollapsed = false, collapseChildren = false) {
        const keySpan = key
            ? `<span class="json-viewer__key">"${UIManager.escapeHtml(key)}"</span><span class="json-viewer__colon">: </span>`
            : '';

        // Header with expand/collapse
        const header = document.createElement('div');
        header.className = 'json-viewer__key-container';
        const expandIcon = startCollapsed ? '▶' : '▼';
        header.innerHTML = `<span class="json-viewer__expand-icon">${expandIcon}</span>${keySpan}<span class="json-viewer__brace">[</span><span class="json-viewer__count"> ${arr.length} items</span>`;

        // Children container
        const childrenContainer = document.createElement('div');
        childrenContainer.className = 'json-viewer__children';
        if (startCollapsed) {
            childrenContainer.style.display = 'none';
        }

        arr.forEach((v, i) => {
            const entry = document.createElement('div');
            entry.className = 'json-viewer__entry';
            // Children inherit the collapseChildren flag as their startCollapsed state
            entry.appendChild(this.createNode(v, String(i), collapseChildren, false));
            childrenContainer.appendChild(entry);
        });

        // Footer
        const footer = document.createElement('div');
        footer.innerHTML = '<span class="json-viewer__brace">]</span>';
        footer.style.marginLeft = '20px';
        if (startCollapsed) {
            footer.style.display = 'none';
        }

        // Toggle functionality
        header.addEventListener('click', () => {
            const isExpanded = childrenContainer.style.display !== 'none';
            childrenContainer.style.display = isExpanded ? 'none' : 'block';
            footer.style.display = isExpanded ? 'none' : 'block';
            header.querySelector('.json-viewer__expand-icon').textContent = isExpanded ? '▶' : '▼';
        });

        container.appendChild(header);
        container.appendChild(childrenContainer);
        container.appendChild(footer);
    }

    /**
     * Render a primitive value (string, number, boolean, null)
     * @param {HTMLElement} container - Container element
     * @param {*} value - Primitive value
     * @param {string|null} key - Object key
     */
    static renderPrimitive(container, value, key) {
        const keySpan = key
            ? `<span class="json-viewer__key">"${UIManager.escapeHtml(key)}"</span><span class="json-viewer__colon">: </span>`
            : '';

        let valueSpan;
        const escapedValue = UIManager.escapeHtml(String(value));

        if (typeof value === 'string') {
            valueSpan = `<span class="json-viewer__string">"${escapedValue}"</span>`;
        } else if (typeof value === 'number') {
            valueSpan = `<span class="json-viewer__number">${value}</span>`;
        } else if (typeof value === 'boolean') {
            valueSpan = `<span class="json-viewer__boolean">${value}</span>`;
        } else if (value === null) {
            valueSpan = `<span class="json-viewer__null">null</span>`;
        } else {
            valueSpan = escapedValue;
        }

        container.innerHTML = `<div style="padding: 1px 0;">${keySpan}${valueSpan}</div>`;
    }
}
