/**
 * Folder Viewer Module
 * Displays data in a folder/file browser style for easier navigation
 * @module folder-viewer
 */

import { UIManager } from './ui-manager.js';

export class FolderViewer {
    /**
     * Render data in folder view
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
        
        // Create folder view structure
        const folderView = document.createElement('div');
        folderView.className = 'folder-viewer';
        
        // Add search/filter bar
        const searchBar = this.createSearchBar();
        folderView.appendChild(searchBar);
        
        // Create main folder tree
        const treeContainer = document.createElement('div');
        treeContainer.className = 'folder-viewer__tree';
        folderView.appendChild(treeContainer);
        
        // Render the data structure
        if (data && typeof data === 'object') {
            // Check if data has a 'data' property (column families)
            if ('data' in data && typeof data.data === 'object') {
                // Create folders for each column family
                const colKeys = Object.keys(data.data);
                colKeys.forEach((colKey, index) => {
                    const colValue = data.data[colKey];
                    if (colValue && typeof colValue === 'object') {
                        // Handle column family with entries array
                        if ('entries' in colValue && Array.isArray(colValue.entries)) {
                            const colFolder = this.createFolder(colKey, colValue.entries, index === 0);
                            treeContainer.appendChild(colFolder);
                        } else {
                            const colFolder = this.createFolder(colKey, colValue, index === 0);
                            treeContainer.appendChild(colFolder);
                        }
                    }
                });
            } else {
                // Direct object structure
                const rootFolder = this.createFolder('database', data, true);
                treeContainer.appendChild(rootFolder);
            }
        } else {
            treeContainer.innerHTML = '<div class="folder-viewer__empty">No data to display</div>';
        }
        
        container.appendChild(folderView);
    }

    /**
     * Create search bar
     */
    static createSearchBar() {
        const searchContainer = document.createElement('div');
        searchContainer.className = 'folder-viewer__search';
        
        const searchInput = document.createElement('input');
        searchInput.type = 'text';
        searchInput.placeholder = '🔍 Search folders and files...';
        searchInput.className = 'folder-viewer__search-input';
        searchInput.addEventListener('input', (e) => {
            this.filterTree(e.target.value);
        });
        
        searchContainer.appendChild(searchInput);
        return searchContainer;
    }

    /**
     * Filter tree based on search query
     */
    static filterTree(query) {
        const tree = document.querySelector('.folder-viewer__tree');
        if (!tree) return;
        
        const lowerQuery = query.toLowerCase();
        const folders = tree.querySelectorAll('.folder-viewer__folder');
        
        folders.forEach(folder => {
            const name = folder.querySelector('.folder-viewer__name')?.textContent.toLowerCase() || '';
            const matches = name.includes(lowerQuery);
            
            // Check if any children match
            const children = folder.querySelectorAll('.folder-viewer__item');
            let hasMatchingChild = false;
            children.forEach(child => {
                const childName = child.querySelector('.folder-viewer__name')?.textContent.toLowerCase() || '';
                if (childName.includes(lowerQuery)) {
                    hasMatchingChild = true;
                    child.style.display = '';
                } else {
                    child.style.display = matches ? '' : 'none';
                }
            });
            
            folder.style.display = (matches || hasMatchingChild) ? '' : 'none';
        });
    }

    /**
     * Create a folder element
     * @param {string} name - Folder name
     * @param {*} data - Folder data
     * @param {boolean} isRoot - Whether this is the root folder
     */
    static createFolder(name, data, isRoot = false) {
        const folder = document.createElement('div');
        folder.className = 'folder-viewer__folder';
        if (isRoot) {
            folder.classList.add('folder-viewer__folder--root');
        }
        
        // Folder header
        const header = document.createElement('div');
        header.className = 'folder-viewer__header';
        header.addEventListener('click', () => this.toggleFolder(folder));
        
        const icon = document.createElement('span');
        icon.className = 'folder-viewer__icon';
        icon.textContent = '📁';
        
        const nameSpan = document.createElement('span');
        nameSpan.className = 'folder-viewer__name';
        nameSpan.textContent = name;
        
        const countSpan = document.createElement('span');
        countSpan.className = 'folder-viewer__count';
        
        // Count items
        let itemCount = 0;
        if (Array.isArray(data)) {
            itemCount = data.length;
        } else if (data && typeof data === 'object') {
            itemCount = Object.keys(data).length;
        }
        countSpan.textContent = `(${itemCount})`;
        
        const expandIcon = document.createElement('span');
        expandIcon.className = 'folder-viewer__expand';
        expandIcon.textContent = '▼';
        
        header.appendChild(icon);
        header.appendChild(nameSpan);
        header.appendChild(countSpan);
        header.appendChild(expandIcon);
        
        // Folder content
        const content = document.createElement('div');
        content.className = 'folder-viewer__content';
        content.style.display = isRoot ? 'block' : 'none';
        
        // Add items
        if (Array.isArray(data)) {
            data.forEach((item, index) => {
                // Check if it's a database entry structure
                if (item && typeof item === 'object' && 'key' in item && 'value' in item) {
                    // Database entry - show key and value
                    const keyStr = typeof item.key === 'object' ? JSON.stringify(item.key) : String(item.key);
                    const entryFolder = this.createFolder(`Entry ${index}: ${keyStr}`, item.value || item);
                    content.appendChild(entryFolder);
                } else {
                    const itemElement = this.createItem(`[${index}]`, item);
                    content.appendChild(itemElement);
                }
            });
        } else if (data && typeof data === 'object') {
            Object.entries(data).forEach(([key, value]) => {
                // Special handling for column families (State, Context, etc.)
                if (key === 'data' && value && typeof value === 'object') {
                    // This is the main data object with column families
                    Object.entries(value).forEach(([colKey, colValue]) => {
                        if (colValue && typeof colValue === 'object' && 'entries' in colValue) {
                            // Column family with entries
                            const colFolder = this.createFolder(colKey, colValue.entries || colValue);
                            content.appendChild(colFolder);
                        } else {
                            const subFolder = this.createFolder(colKey, colValue);
                            content.appendChild(subFolder);
                        }
                    });
                } else if (value && typeof value === 'object' && !Array.isArray(value)) {
                    // Check if it's a column family structure
                    if ('entries' in value || 'count' in value) {
                        // Column family structure
                        const entries = value.entries || [];
                        const colFolder = this.createFolder(key, entries);
                        content.appendChild(colFolder);
                    } else {
                        // Nested object - create subfolder
                        const subFolder = this.createFolder(key, value);
                        content.appendChild(subFolder);
                    }
                } else if (Array.isArray(value)) {
                    // Array - create folder with array icon
                    const arrayFolder = this.createFolder(key, value);
                    const icon = arrayFolder.querySelector('.folder-viewer__icon');
                    if (icon) {
                        icon.textContent = '📋';
                    }
                    content.appendChild(arrayFolder);
                } else {
                    // Primitive value - create file
                    const itemElement = this.createItem(key, value);
                    content.appendChild(itemElement);
                }
            });
        }
        
        folder.appendChild(header);
        folder.appendChild(content);
        
        return folder;
    }

    /**
     * Create a file/item element
     * @param {string} name - Item name
     * @param {*} value - Item value
     */
    static createItem(name, value) {
        const item = document.createElement('div');
        item.className = 'folder-viewer__item';
        
        const icon = document.createElement('span');
        icon.className = 'folder-viewer__icon';
        
        const nameSpan = document.createElement('span');
        nameSpan.className = 'folder-viewer__name';
        nameSpan.textContent = name;
        
        const valueSpan = document.createElement('span');
        valueSpan.className = 'folder-viewer__value';
        
        // Format value based on type
        if (value === null) {
            icon.textContent = '📄';
            valueSpan.textContent = 'null';
            valueSpan.classList.add('folder-viewer__value--null');
        } else if (typeof value === 'string') {
            icon.textContent = '📄';
            const displayValue = value.length > 100 ? value.substring(0, 100) + '...' : value;
            valueSpan.textContent = `"${displayValue}"`;
            valueSpan.classList.add('folder-viewer__value--string');
        } else if (typeof value === 'number') {
            icon.textContent = '🔢';
            valueSpan.textContent = value;
            valueSpan.classList.add('folder-viewer__value--number');
        } else if (typeof value === 'boolean') {
            icon.textContent = '✓';
            valueSpan.textContent = value;
            valueSpan.classList.add('folder-viewer__value--boolean');
        } else {
            icon.textContent = '📄';
            valueSpan.textContent = JSON.stringify(value);
        }
        
        // Add click handler to show full value in a modal or expand
        item.addEventListener('click', () => {
            this.showValueDetails(name, value);
        });
        
        item.appendChild(icon);
        item.appendChild(nameSpan);
        item.appendChild(valueSpan);
        
        return item;
    }

    /**
     * Toggle folder expand/collapse
     */
    static toggleFolder(folder) {
        const content = folder.querySelector('.folder-viewer__content');
        const expandIcon = folder.querySelector('.folder-viewer__expand');
        
        if (content.style.display === 'none') {
            content.style.display = 'block';
            expandIcon.textContent = '▼';
        } else {
            content.style.display = 'none';
            expandIcon.textContent = '▶';
        }
    }

    /**
     * Show value details in a modal or expanded view
     */
    static showValueDetails(name, value) {
        // Create a simple modal or expand the item
        const modal = document.createElement('div');
        modal.className = 'folder-viewer__modal';
        
        const modalContent = document.createElement('div');
        modalContent.className = 'folder-viewer__modal-content';
        
        const header = document.createElement('div');
        header.className = 'folder-viewer__modal-header';
        header.innerHTML = `
            <h3>${UIManager.escapeHtml(name)}</h3>
            <button class="folder-viewer__modal-close">×</button>
        `;
        
        const body = document.createElement('div');
        body.className = 'folder-viewer__modal-body';
        
        if (typeof value === 'object' && value !== null) {
            body.textContent = JSON.stringify(value, null, 2);
            body.style.fontFamily = 'monospace';
            body.style.whiteSpace = 'pre-wrap';
        } else {
            body.textContent = String(value);
        }
        
        modalContent.appendChild(header);
        modalContent.appendChild(body);
        modal.appendChild(modalContent);
        
        // Close handlers
        const closeBtn = header.querySelector('.folder-viewer__modal-close');
        closeBtn.addEventListener('click', () => modal.remove());
        modal.addEventListener('click', (e) => {
            if (e.target === modal) {
                modal.remove();
            }
        });
        
        document.body.appendChild(modal);
    }
}

