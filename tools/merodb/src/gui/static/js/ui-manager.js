/**
 * UI Manager Module
 * Handles DOM manipulation and UI state
 * @module ui-manager
 */

export class UIManager {
    /**
     * Show an element by removing the 'hidden' class
     * @param {string} id - Element ID
     */
    static showElement(id) {
        const element = document.getElementById(id);
        if (element) {
            element.classList.remove('hidden');
        }
    }

    /**
     * Hide an element by adding the 'hidden' class
     * @param {string} id - Element ID
     */
    static hideElement(id) {
        const element = document.getElementById(id);
        if (element) {
            element.classList.add('hidden');
        }
    }

    /**
     * Show a message in an element
     * @param {string} id - Element ID
     * @param {string} message - Message text
     */
    static showMessage(id, message) {
        const element = document.getElementById(id);
        if (element) {
            element.textContent = message;
            element.classList.remove('hidden');
        }
    }

    /**
     * Hide a message element
     * @param {string} id - Element ID
     */
    static hideMessage(id) {
        this.hideElement(id);
    }

    /**
     * Show the viewer UI (hide upload form)
     */
    static showViewer() {
        this.hideElement('upload-view');
        this.showElement('viewer-view');
    }

    /**
     * Hide the viewer UI
     */
    static hideViewer() {
        this.hideElement('viewer-view');
    }

    /**
     * Show the upload UI (hide viewer)
     */
    static showUpload() {
        this.showElement('upload-view');
        this.hideElement('viewer-view');
    }

    /**
     * Toggle active state on tab buttons
     * @param {string} activeTab - Tab name to activate
     */
    static setActiveTab(activeTab) {
        document.querySelectorAll('.tabs__button').forEach(btn => {
            const isActive = btn.dataset.tab === activeTab;
            btn.classList.toggle('tabs__button--active', isActive);
        });
    }

    /**
     * Escape HTML special characters
     * @param {string} str - String to escape
     * @returns {string} Escaped string
     */
    static escapeHtml(str) {
        return String(str)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#039;');
    }

    /**
     * Create an element with classes
     * @param {string} tag - HTML tag name
     * @param {string|string[]} classes - Class name(s)
     * @param {Object} attributes - Optional attributes
     * @returns {HTMLElement}
     */
    static createElement(tag, classes = [], attributes = {}) {
        const element = document.createElement(tag);

        const classList = Array.isArray(classes) ? classes : [classes];
        classList.forEach(className => {
            if (className) element.classList.add(className);
        });

        Object.entries(attributes).forEach(([key, value]) => {
            element.setAttribute(key, value);
        });

        return element;
    }
}
