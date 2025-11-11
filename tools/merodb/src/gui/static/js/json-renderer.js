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
     */
    static render(data, containerId) {
        const container = document.getElementById(containerId);
        if (!container) {
            console.error(`Container ${containerId} not found`);
            return;
        }

        container.innerHTML = '';
        container.appendChild(this.createNode(data));
    }

    /**
     * Create a DOM node for a JSON value
     * @param {*} value - JSON value
     * @param {string|null} key - Object key or array index
     * @returns {HTMLElement}
     */
    static createNode(value, key = null) {
        const div = document.createElement('div');
        div.className = 'json-viewer__node';

        if (Array.isArray(value)) {
            this.renderArray(div, value, key);
        } else if (value !== null && typeof value === 'object') {
            this.renderObject(div, value, key);
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
     */
    static renderObject(container, obj, key) {
        const entries = Object.entries(obj);
        const keySpan = key
            ? `<span class="json-viewer__key">"${UIManager.escapeHtml(key)}"</span><span class="json-viewer__colon">: </span>`
            : '';

        // Header with expand/collapse
        const header = document.createElement('div');
        header.className = 'json-viewer__key-container';
        header.innerHTML = `<span class="json-viewer__expand-icon">▼</span>${keySpan}<span class="json-viewer__brace">{</span><span class="json-viewer__count"> ${entries.length} items</span>`;

        // Children container
        const childrenContainer = document.createElement('div');
        childrenContainer.className = 'json-viewer__children';

        entries.forEach(([k, v]) => {
            const entry = document.createElement('div');
            entry.className = 'json-viewer__entry';
            entry.appendChild(this.createNode(v, k));
            childrenContainer.appendChild(entry);
        });

        // Footer
        const footer = document.createElement('div');
        footer.innerHTML = '<span class="json-viewer__brace">}</span>';
        footer.style.marginLeft = '20px';

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
     */
    static renderArray(container, arr, key) {
        const keySpan = key
            ? `<span class="json-viewer__key">"${UIManager.escapeHtml(key)}"</span><span class="json-viewer__colon">: </span>`
            : '';

        // Header with expand/collapse
        const header = document.createElement('div');
        header.className = 'json-viewer__key-container';
        header.innerHTML = `<span class="json-viewer__expand-icon">▼</span>${keySpan}<span class="json-viewer__brace">[</span><span class="json-viewer__count"> ${arr.length} items</span>`;

        // Children container
        const childrenContainer = document.createElement('div');
        childrenContainer.className = 'json-viewer__children';

        arr.forEach((v, i) => {
            const entry = document.createElement('div');
            entry.className = 'json-viewer__entry';
            entry.appendChild(this.createNode(v, String(i)));
            childrenContainer.appendChild(entry);
        });

        // Footer
        const footer = document.createElement('div');
        footer.innerHTML = '<span class="json-viewer__brace">]</span>';
        footer.style.marginLeft = '20px';

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
