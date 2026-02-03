/**
 * State Tree Visualizer Module
 * Renders hierarchical state tree using D3.js
 * @module state-tree-visualizer
 */

import { ApiService } from './api-service.js';
import { UIManager } from './ui-manager.js';
import { TooltipManager } from './tooltip-manager.js';

export class StateTreeVisualizer {
    /**
     * @param {import('./app-state.js').AppState} state - Application state
     * @param {string} svgId - Optional SVG element ID (defaults to 'state-svg')
     */
    constructor(state, svgId = 'state-svg') {
        this.state = state;
        this.currentZoom = null;
        this.root = null;
        this.updateFn = null;
        this.tooltipManager = new TooltipManager();
        this.svgId = svgId;
    }

    /**
     * Check if a node has decoded state data
     * @param {Object} d - D3 node data
     * @returns {boolean} True if node has decoded data
     */
    hasDecodedData(d) {
        return d.data.data && (d.data.data.key || d.data.data.value || d.data.data.field);
    }

    /**
     * Load state tree data from backend
     * Uses the new paginated API: first loads context list, then loads trees on-demand
     */
    async load() {
        // Check if we have schema content (from file or local storage)
        // Schema is optional - if not provided, backend will infer it from database
        if (!this.state.currentStateSchemaFile && !this.state.currentStateSchemaFileContent) {
            // Try to load from local storage
            try {
                const savedContent = localStorage.getItem('merodb_schema_content');
                if (savedContent) {
                    this.state.currentStateSchemaFileContent = savedContent;
                    console.log('[StateTreeVisualizer] Loaded schema from local storage');
                } else {
                    console.log('[StateTreeVisualizer] No schema file provided - will use schema inference');
                }
            } catch (err) {
                console.log('[StateTreeVisualizer] No schema file provided - will use schema inference');
            }
        }

        UIManager.showElement('state-loading');
        UIManager.hideElement('state-error');

        try {
            // First, list available contexts (fast operation)
            const contextsResponse = await ApiService.listContexts(this.state.currentDbPath);

            // Initialize state with context list
            if (!this.state.stateTreeData) {
                this.state.stateTreeData = {
                    contexts: [],
                    loadedTrees: new Map() // Cache loaded trees by context_id
                };
            }

            // Store context metadata
            this.state.stateTreeData.contexts = contextsResponse.data.contexts;

            // Populate dropdown with contexts
            this.populateContextSelector();

            // Load the first context's tree automatically
            if (this.state.stateTreeData.contexts.length > 0) {
                const firstContextId = this.state.stateTreeData.contexts[0].context_id;
                await this.loadContextTree(firstContextId);
            }

            this.render();
        } catch (error) {
            throw error;
        } finally {
            UIManager.hideElement('state-loading');
        }
    }

    /**
     * Load tree for a specific context (on-demand)
     * @param {string} contextId - Context ID to load
     */
    async loadContextTree(contextId) {
        // Check if already loaded
        if (this.state.stateTreeData?.loadedTrees?.has(contextId)) {
            return;
        }

        // Only show loading indicator for main visualizer (not additional)
        if (this.svgId === 'state-svg') {
            UIManager.showElement('state-loading');
        }

        try {
            // Use cached schema content if file is not available (e.g., after refresh)
            const schemaFile = this.state.currentStateSchemaFile;
            const schemaContent = this.state.currentStateSchemaFileContent;
            
            // Debug: Log what we're sending
            console.log('Loading context tree:', {
                contextId,
                svgId: this.svgId,
                hasStateSchemaFile: !!schemaFile,
                hasSchemaContent: !!schemaContent,
                stateSchemaFileName: schemaFile?.name || this.state.currentStateSchemaFileName || 'none'
            });
            
            const response = await ApiService.loadContextTree(
                this.state.currentDbPath,
                contextId,
                schemaFile || (schemaContent ? new File([schemaContent], this.state.currentStateSchemaFileName || 'state-schema.json', { type: 'application/json' }) : null)
            );

            // Cache the loaded tree
            if (!this.state.stateTreeData.loadedTrees) {
                this.state.stateTreeData.loadedTrees = new Map();
            }
            this.state.stateTreeData.loadedTrees.set(contextId, response.data.tree);
        } catch (error) {
            console.error(`Failed to load tree for context ${contextId}:`, error);
            throw error;
        } finally {
            // Only hide loading indicator for main visualizer (not additional)
            if (this.svgId === 'state-svg') {
                UIManager.hideElement('state-loading');
            }
        }
    }

    /**
     * Populate context selector dropdown with available contexts
     */
    populateContextSelector() {
        // Only populate the main context selector, not for additional visualizers
        if (this.svgId !== 'state-svg') {
            return;
        }
        const select = document.getElementById('state-context-select');
        if (!select || !this.state.stateTreeData?.contexts) return;

        // Clear existing options
        select.innerHTML = '';

        // Add option for each context
        this.state.stateTreeData.contexts.forEach((context, index) => {
            const option = document.createElement('option');
            option.value = context.context_id;
            option.textContent = `Context ${index + 1}: ${context.context_id.substring(0, 8)}... (${context.root_hash.substring(0, 8)}...)`;
            select.appendChild(option);
        });

        // Add change event listener to load tree on-demand when context changes
        select.removeEventListener('change', this._contextChangeHandler);
        this._contextChangeHandler = async (e) => {
            const contextId = e.target.value;

            // Store the context ID being loaded to handle race conditions
            this.pendingContextId = contextId;

            try {
                await this.loadContextTree(contextId);

                // Only render if this is still the selected context (no race condition)
                if (this.pendingContextId === contextId && e.target.value === contextId) {
                    this.render();
                }
            } catch (error) {
                // Only show error if this is still the selected context
                if (this.pendingContextId === contextId && e.target.value === contextId) {
                    UIManager.showError('state-error', `Failed to load context tree: ${error.message}`);
                }
            }
        };
        select.addEventListener('change', this._contextChangeHandler);

        // Select the first context by default
        if (this.state.stateTreeData.contexts.length > 0) {
            select.value = this.state.stateTreeData.contexts[0].context_id;
        }
    }

    /**
     * Get the tree data for the currently selected context
     * @returns {Object|null} Tree data for d3.hierarchy
     */
    getSelectedContextTree() {
        if (!this.state.stateTreeData?.contexts) return null;

        // For additional visualizer, use the first (and only) loaded context
        if (this.svgId !== 'state-svg') {
            // Get the first (and likely only) context from loadedTrees
            const loadedTrees = this.state.stateTreeData.loadedTrees;
            if (loadedTrees && loadedTrees.size > 0) {
                const firstContextId = Array.from(loadedTrees.keys())[0];
                const tree = loadedTrees.get(firstContextId);
                if (tree) {
                    try {
                        return JSON.parse(JSON.stringify(tree));
                    } catch (e) {
                        console.error('[StateTreeVisualizer] Failed to clone tree:', e);
                        return null;
                    }
                }
            }
            return null;
        }

        const select = document.getElementById('state-context-select');
        const selectedContextId = select?.value;

        if (!selectedContextId) return null;

        // Verify this matches the pending context (prevent rendering outdated data)
        if (this.pendingContextId && this.pendingContextId !== selectedContextId) {
            return null;
        }

        // Get from cache
        const tree = this.state.stateTreeData.loadedTrees?.get(selectedContextId);

        // Deep clone to prevent mutation of original stateTreeData
        if (!tree) {
            console.warn('[StateTreeVisualizer] No tree data found for context:', selectedContextId);
            return null;
        }

        try {
            const cloned = JSON.parse(JSON.stringify(tree));
            console.log('[StateTreeVisualizer] Tree structure:', {
                hasId: !!cloned.id,
                hasType: !!cloned.type,
                hasChildren: !!cloned.children,
                childrenCount: cloned.children?.length || 0,
                firstChild: cloned.children?.[0] ? {
                    id: cloned.children[0].id,
                    type: cloned.children[0].type,
                    field: cloned.children[0].field,
                    hasChildren: !!cloned.children[0].children,
                    childrenCount: cloned.children[0].children?.length || 0
                } : null
            });
            return cloned;
        } catch (error) {
            console.error(`Failed to clone tree for context ${selectedContextId}:`, error);
            return {
                id: 'clone-error',
                type: 'Error',
                error: 'Failed to clone tree data'
            };
        }
    }

    /**
     * Render tree based on selected layout
     */
    render() {
        // Clean up existing tooltips before re-render to prevent memory leaks
        this.tooltipManager.cleanupTooltips();

        const layout = document.getElementById('state-layout-select')?.value || 'folder';

        if (layout === 'tree') {
            this.renderTree();
        } else if (layout === 'folder') {
            this.renderFolder();
        }

        this.updateStats();
    }

    /**
     * Render top-down tree layout with collapsible nodes
     */
    renderTree() {
        const svg = d3.select(`#${this.svgId}`);
        svg.selectAll('*').remove();

        const svgNode = svg.node();
        if (!svgNode) {
            console.error(`SVG element with id "${this.svgId}" not found`);
            return;
        }
        const width = svgNode.getBoundingClientRect().width;
        const height = 600;
        svg.attr('viewBox', [0, 0, width, height]);

        const g = svg.append('g')
            .attr('transform', 'translate(50,50)');

        // Convert data to D3 hierarchy
        const hierarchyData = this.getSelectedContextTree();
        if (!hierarchyData) {
            console.error('No tree data available for selected context');
            return;
        }
        const root = d3.hierarchy(hierarchyData);

        // Initialize tree with all nodes collapsed except root's children
        root.descendants().forEach((d, i) => {
            d.id = i;
            d._children = d.children;
            if (d.depth > 1) {
                d.children = null;
            }
        });

        this.root = root;
        root.x0 = height / 2;
        root.y0 = 0;

        const treeLayout = d3.tree().size([height - 100, width - 200]);

        const update = (source) => {
            const duration = 250;
            const nodes = root.descendants();
            const links = root.links();

            // Compute the new tree layout
            treeLayout(root);

            // Update nodes
            const node = g.selectAll('.state-node')
                .data(nodes, d => d.id);

            // Enter new nodes at parent's previous position
            const nodeEnter = node.enter().append('g')
                .attr('class', 'state-node')
                .attr('transform', d => `translate(${source.y0},${source.x0})`)
                .style('cursor', d => d._children || d.children ? 'pointer' : 'default')
                .on('click', (event, d) => {
                    // Only expand/collapse if Cmd/Ctrl is NOT pressed
                    if (!event.metaKey && !event.ctrlKey) {
                        if (d.children) {
                            d._children = d.children;
                            d.children = null;
                        } else if (d._children) {
                            d.children = d._children;
                            d._children = null;
                        }
                        update(d);
                        this.updateStats();
                    }
                })
                .on('mouseover', (event, d) => {
                    const content = this.formatTooltipContent(d);
                    this.tooltipManager.showTooltip(event, content, 'state-tooltip-temp');
                })
                .on('mousemove', (event) => {
                    this.tooltipManager.moveTooltip(event);
                })
                .on('mouseout', () => {
                    this.tooltipManager.hideTooltip();
                });

            nodeEnter.append('circle')
                .attr('r', 6)
                .attr('class', d => {
                    if (this.hasDecodedData(d)) return 'has-data';
                    if (!d.children && !d._children) return 'leaf';
                    return '';
                });

            // Add node ID labels
            nodeEnter.append('text')
                .attr('dy', '0.31em')
                .attr('x', d => (d.children || d._children) ? -10 : 10)
                .attr('text-anchor', d => (d.children || d._children) ? 'end' : 'start')
                .text(d => {
                    const id = d.data.id || 'N/A';
                    return id !== 'N/A' ? `${id.substring(0, 8)}...` : 'N/A';
                })
                .style('font-size', '10px')
                .style('fill', '#bbb')
                .style('pointer-events', 'none');

            // Transition nodes to their new position
            const nodeUpdate = nodeEnter.merge(node);

            nodeUpdate.transition()
                .duration(duration)
                .attr('transform', d => `translate(${d.y},${d.x})`);

            nodeUpdate.select('circle')
                .attr('class', d => {
                    if (this.hasDecodedData(d)) return 'has-data';
                    if (!d.children && !d._children) return 'leaf';
                    if (d._children) return 'collapsed';
                    return '';
                });

            nodeUpdate.select('text')
                .attr('x', d => (d.children || d._children) ? -10 : 10)
                .attr('text-anchor', d => (d.children || d._children) ? 'end' : 'start');

            // Transition exiting nodes
            const nodeExit = node.exit()
                .transition()
                .duration(duration)
                .attr('transform', d => `translate(${source.y},${source.x})`)
                .remove();

            // Update links
            const link = g.selectAll('.state-link')
                .data(links, d => d.target.id);

            // Enter new links at parent's previous position
            const linkEnter = link.enter().insert('path', 'g')
                .attr('class', 'state-link')
                .attr('d', d => {
                    const o = { x: source.x0, y: source.y0 };
                    return diagonal(o, o);
                });

            // Transition links to their new position
            linkEnter.merge(link)
                .transition()
                .duration(duration)
                .attr('d', d => diagonal(d.source, d.target));

            // Transition exiting links
            link.exit()
                .transition()
                .duration(duration)
                .attr('d', d => {
                    const o = { x: source.x, y: source.y };
                    return diagonal(o, o);
                })
                .remove();

            // Store old positions for transition
            nodes.forEach(d => {
                d.x0 = d.x;
                d.y0 = d.y;
            });
        };

        // Diagonal path generator for links
        const diagonal = (s, d) => {
            return `M ${s.y} ${s.x}
                    C ${(s.y + d.y) / 2} ${s.x},
                      ${(s.y + d.y) / 2} ${d.x},
                      ${d.y} ${d.x}`;
        };

        this.updateFn = update;
        update(root);
        this.setupZoom(svg, g);
    }

    /**
     * Setup D3 zoom behavior
     * @param {d3.Selection} svg - SVG element
     * @param {d3.Selection} g - Group element
     */
    setupZoom(svg, g) {
        // Get the initial transform from the group element
        const initialTransform = g.attr('transform') || '';

        const zoom = d3.zoom()
            .scaleExtent([0.1, 10])
            .on('zoom', (event) => {
                // Apply zoom transform while preserving the initial transform
                const t = event.transform;
                g.attr('transform', `${initialTransform} translate(${t.x},${t.y}) scale(${t.k})`);
            });

        svg.call(zoom);
        this.currentZoom = zoom;
    }

    /**
     * Reset zoom to identity
     */
    resetZoom() {
        if (this.currentZoom) {
            d3.select(`#${this.svgId}`)
                .transition()
                .duration(750)
                .call(this.currentZoom.transform, d3.zoomIdentity);
        }
    }

    /**
     * Expand all nodes in the tree
     */
    expandAll() {
        if (!this.root) return;

        // Recursive function to expand a node and all its descendants
        const expandNode = (d) => {
            if (d._children) {
                d.children = d._children;
                d._children = null;
            }
            // Recursively expand all children
            if (d.children) {
                d.children.forEach(expandNode);
            }
        };

        // Start expanding from root
        expandNode(this.root);

        if (this.updateFn) {
            this.updateFn(this.root);
        } else {
            this.render();
        }
        this.updateStats();
    }

    /**
     * Collapse entire tree to root only
     */
    collapseAll() {
        if (!this.root) return;

        const collapseNode = (d) => {
            if (d.children) {
                d._children = d.children;
                d._children.forEach(collapseNode);
                d.children = null;
            }
        };

        // Collapse all children of root (same as clicking on root node)
        if (this.root.children) {
            this.root._children = this.root.children;
            this.root._children.forEach(collapseNode);
            this.root.children = null;
        }

        if (this.updateFn) {
            this.updateFn(this.root);
        } else {
            this.render();
        }
        this.updateStats();
    }

    /**
     * Update statistics display
     */
    updateStats() {
        // Use different stats containers for main vs additional visualizers
        const statsId = this.svgId === 'state-svg' ? 'state-stats' : 'state-additional-stats';
        const container = document.getElementById(statsId);
        if (!container || !this.root) return;

        const nodeCount = this.root.descendants().length;
        const leafCount = this.root.leaves().length;
        const maxDepth = this.root.height;

        container.innerHTML = `
            <div class="stats__item">
                <div class="stats__label">Nodes</div>
                <div class="stats__value">${nodeCount}</div>
            </div>
            <div class="stats__item">
                <div class="stats__label">Leaves</div>
                <div class="stats__value">${leafCount}</div>
            </div>
            <div class="stats__item">
                <div class="stats__label">Depth</div>
                <div class="stats__value">${maxDepth}</div>
            </div>
        `;
    }

    /**
     * Export state tree as SVG file
     */
    exportImage() {
        const svg = document.getElementById(this.svgId);
        if (!svg) return;

        const serializer = new XMLSerializer();
        const source = serializer.serializeToString(svg);
        const blob = new Blob([source], { type: 'image/svg+xml;charset=utf-8' });
        const url = URL.createObjectURL(blob);

        const link = document.createElement('a');
        link.href = url;
        link.download = 'state-tree-visualization.svg';
        link.click();

        URL.revokeObjectURL(url);
    }

    /**
     * Format tooltip content HTML
     * @param {Object} node - D3 hierarchy node object with data and parent properties
     * @returns {string} HTML content
     */
    formatTooltipContent(node) {
        const data = node.data;

        let html = '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Node Information</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Type:</span>`;
        html += `  <span class="visualization__tooltip-value">${data.type || 'N/A'}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Children:</span>`;
        html += `  <span class="visualization__tooltip-value">${data.children_count || 0}</span>`;
        html += `</div>`;
        html += `</div>`;

        // Display decoded state data if available
        if (data.data) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Decoded State</div>`;

            const stateData = data.data;

            // Display field name if available
            if (stateData.field) {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Field:</span>`;
                html += `  <span class="visualization__tooltip-value">${stateData.field}</span>`;
                html += `</div>`;
            }

            // Display key-value for Entry types
            if (stateData.key && stateData.value) {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Key:</span>`;
                html += `  <span class="visualization__tooltip-value"><pre>${JSON.stringify(stateData.key.parsed, null, 2)}</pre></span>`;
                html += `</div>`;
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Value:</span>`;
                html += `  <span class="visualization__tooltip-value"><pre>${JSON.stringify(stateData.value.parsed, null, 2)}</pre></span>`;
                html += `</div>`;
            }
            // Display value for ScalarEntry types
            else if (stateData.value) {
                const value = stateData.value.parsed || stateData.value;
                
                // Special handling for Counter values
                if (value && typeof value === 'object' && (value.crdt_type === 'GCounter' || value.crdt_type === 'PNCounter')) {
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Counter Type:</span>`;
                    html += `  <span class="visualization__tooltip-value">${value.crdt_type}</span>`;
                    html += `</div>`;
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Total Value:</span>`;
                    html += `  <span class="visualization__tooltip-value"><strong>${value.value}</strong></span>`;
                    html += `</div>`;
                    
                    // Display positive map entries
                    if (value.positive && value.positive.entries) {
                        html += `<div class="visualization__tooltip-row">`;
                        html += `  <span class="visualization__tooltip-label">Positive Entries:</span>`;
                        html += `  <span class="visualization__tooltip-value">`;
                        html += `    <table style="margin-top: 8px; border-collapse: collapse; width: 100%;">`;
                        html += `      <thead><tr><th style="text-align: left; padding: 4px; border-bottom: 1px solid #ddd;">Executor ID</th><th style="text-align: right; padding: 4px; border-bottom: 1px solid #ddd;">Value</th></tr></thead>`;
                        html += `      <tbody>`;
                        for (const [executorId, entry] of Object.entries(value.positive.entries)) {
                            html += `        <tr>`;
                            html += `          <td style="padding: 4px; font-family: monospace; font-size: 11px;">${UIManager.escapeHtml(executorId)}</td>`;
                            html += `          <td style="text-align: right; padding: 4px;">${entry.value}</td>`;
                            html += `        </tr>`;
                        }
                        html += `      </tbody>`;
                        html += `      <tfoot><tr><td style="padding: 4px; border-top: 1px solid #ddd; font-weight: bold;">Total</td><td style="text-align: right; padding: 4px; border-top: 1px solid #ddd; font-weight: bold;">${value.positive.total}</td></tr></tfoot>`;
                        html += `    </table>`;
                        html += `  </span>`;
                        html += `</div>`;
                    }
                    
                    // Display negative map entries (for PNCounter)
                    if (value.negative && value.negative.entries) {
                        html += `<div class="visualization__tooltip-row">`;
                        html += `  <span class="visualization__tooltip-label">Negative Entries:</span>`;
                        html += `  <span class="visualization__tooltip-value">`;
                        html += `    <table style="margin-top: 8px; border-collapse: collapse; width: 100%;">`;
                        html += `      <thead><tr><th style="text-align: left; padding: 4px; border-bottom: 1px solid #ddd;">Executor ID</th><th style="text-align: right; padding: 4px; border-bottom: 1px solid #ddd;">Value</th></tr></thead>`;
                        html += `      <tbody>`;
                        for (const [executorId, entry] of Object.entries(value.negative.entries)) {
                            html += `        <tr>`;
                            html += `          <td style="padding: 4px; font-family: monospace; font-size: 11px;">${UIManager.escapeHtml(executorId)}</td>`;
                            html += `          <td style="text-align: right; padding: 4px;">${entry.value}</td>`;
                            html += `        </tr>`;
                        }
                        html += `      </tbody>`;
                        html += `      <tfoot><tr><td style="padding: 4px; border-top: 1px solid #ddd; font-weight: bold;">Total</td><td style="text-align: right; padding: 4px; border-top: 1px solid #ddd; font-weight: bold;">${value.negative.total}</td></tr></tfoot>`;
                        html += `    </table>`;
                        html += `  </span>`;
                        html += `</div>`;
                    }
                } else {
                    // Regular value display
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Value:</span>`;
                    html += `  <span class="visualization__tooltip-value"><pre>${JSON.stringify(value, null, 2)}</pre></span>`;
                    html += `</div>`;
                }
            }

            html += `</div>`;
        }

        html += '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Hashes</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">ID:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(data.id, 'ID')}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Full Hash:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(data.full_hash, 'Full Hash')}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Own Hash:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(data.own_hash, 'Own Hash')}</span>`;
        html += `</div>`;
        // Use the parent node's ID from the D3 hierarchy instead of data.parent_id
        // This ensures the displayed parent ID matches what's shown in the tree
        if (node.parent) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Parent ID:</span>`;
            html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(node.parent.data.id, 'Parent ID')}</span>`;
            html += `</div>`;
        }
        html += `</div>`;

        html += '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Timestamps</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Created:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(data.created_at)}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Updated:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(data.updated_at)}</span>`;
        html += `</div>`;
        if (data.deleted_at) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Deleted:</span>`;
            html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(data.deleted_at)}</span>`;
            html += `</div>`;
        }
        html += `</div>`;

        return html;
    }

    /**
     * Render folder/file tree view using D3.js
     * Displays the state tree as a file browser with folders and files
     */
    renderFolder() {
        const svg = d3.select(`#${this.svgId}`);
        svg.selectAll('*').remove();

        const svgNode = svg.node();
        if (!svgNode) {
            console.error(`SVG element with id "${this.svgId}" not found`);
            return;
        }
        const width = svgNode.getBoundingClientRect().width;
        const height = 600;
        svg.attr('viewBox', [0, 0, width, height]);

        const g = svg.append('g')
            .attr('transform', 'translate(20,20)');

        // Get tree data
        const hierarchyData = this.getSelectedContextTree();
        if (!hierarchyData) {
            console.error('No tree data available for selected context');
            return;
        }

        const root = d3.hierarchy(hierarchyData);
        
        // Initialize tree with all nodes collapsed except root's children
        root.descendants().forEach((d, i) => {
            d.id = i;
            d._children = d.children;
            if (d.depth > 1) {
                d.children = null;
            }
        });

        this.root = root;
        
        // Calculate layout - vertical file tree with dynamic heights
        const baseNodeHeight = 20;
        const indent = 20;
        
        let y = 0;
        const nodes = [];
        
        const calculateLayout = (node) => {
            // Calculate height based on content length
            const data = node.data;
            let contentLength = 0;
            if (data.data) {
                const stateData = data.data;
                if (stateData.value && stateData.value.parsed !== undefined) {
                    contentLength = JSON.stringify(stateData.value.parsed).length;
                } else if (stateData.key && stateData.key.parsed !== undefined) {
                    contentLength = JSON.stringify(stateData.key.parsed).length;
                }
            }
            // Estimate lines needed (assuming ~80 chars per line)
            const estimatedLines = Math.max(1, Math.ceil(contentLength / 80));
            const nodeHeight = baseNodeHeight + (estimatedLines - 1) * 14;
            
            node.y = y;
            node.x = node.depth * indent;
            node._height = nodeHeight; // Store height for rendering
            nodes.push(node);
            y += nodeHeight;
            
            if (node.children) {
                node.children.forEach(calculateLayout);
            }
        };
        
        calculateLayout(root);

        // Draw very subtle connection lines (similar to tree view)
        const links = g.append('g').attr('class', 'folder-links');
        nodes.forEach(node => {
            if (node.parent && nodes.includes(node.parent)) {
                const parentHeight = node.parent._height || baseNodeHeight;
                // Draw a very subtle vertical line from parent to child
                links.append('line')
                    .attr('x1', node.parent.x)
                    .attr('y1', node.parent.y + parentHeight)
                    .attr('x2', node.x)
                    .attr('y2', node.y)
                    .attr('stroke', '#555')
                    .attr('stroke-width', 0.5)
                    .attr('opacity', 0.2);
            }
        });

        // Draw nodes as folder/file icons
        const nodeGroup = g.append('g').attr('class', 'folder-nodes');
        
        const nodesEnter = nodeGroup.selectAll('.folder-node')
            .data(nodes)
            .enter()
            .append('g')
            .attr('class', 'folder-node')
            .attr('transform', d => `translate(${d.x},${d.y})`)
            .style('cursor', 'pointer');

        // Add circles only for leaf nodes (last elements)
        nodesEnter.filter(d => !d.children && !d._children)
            .append('circle')
            .attr('r', 4)
            .attr('cx', 0)
            .attr('cy', d => (d._height || baseNodeHeight) / 2)
            .attr('fill', d => {
                const data = d.data;
                // Check if deleted
                if (data.deleted_at) {
                    return '#9E9E9E'; // Gray for deleted items
                }
                if (this.hasDecodedData(d)) return '#4CAF50'; // Green for nodes with data
                return '#2196F3'; // Blue for leaves
            })
            .attr('stroke', d => {
                const data = d.data;
                return data.deleted_at ? '#666' : '#666';
            })
            .attr('stroke-width', 1)
            .attr('opacity', d => {
                const data = d.data;
                return data.deleted_at ? 0.5 : 1.0; // Reduced opacity for deleted items
            });

        // Add node label - show full values with text wrapping
        nodesEnter.each(function(d) {
            const nodeHeight = d._height || baseNodeHeight;
            const g = d3.select(this);
            const data = d.data;
            
            // Check if item is deleted
            const isDeleted = data.deleted_at !== null && data.deleted_at !== undefined;
            
            // Create text element that can wrap
            const text = g.append('text')
                .attr('x', (!d.children && !d._children) ? 8 : 0) // Offset for leaf nodes with circles
                .attr('y', nodeHeight / 2)
                .attr('dy', '0.35em')
                .attr('font-size', '11px')
                .attr('fill', isDeleted ? '#888' : '#d4d4d4') // Grayed out for deleted
                .attr('opacity', isDeleted ? 0.6 : 1.0); // Reduced opacity for deleted
            
            let labelText = '';
            
            // For Field types (collapsible), show field name with type information
            if (data.type === 'Field') {
                const fieldName = data.field || 'Field';
                let typeInfo = '';
                
                // Get type information from data
                if (data.crdt_type) {
                    typeInfo = data.crdt_type;
                } else if (data.type_info) {
                    typeInfo = data.type_info;
                } else if (data.data && data.data.crdt_type) {
                    typeInfo = data.data.crdt_type;
                } else if (data.data && data.data.type) {
                    typeInfo = data.data.type;
                }
                
                // Check if this is a Counter field and show the total value
                let counterValue = null;
                if (data.data && data.data.value) {
                    const value = data.data.value.parsed || data.data.value;
                    if (value && typeof value === 'object' && (value.crdt_type === 'GCounter' || value.crdt_type === 'PNCounter')) {
                        counterValue = value.value;
                    }
                }
                
                // Format type info nicely
                if (typeInfo) {
                    // Convert common type names to readable format
                    const typeMap = {
                        'UnorderedMap': 'unordered_map',
                        'UnorderedSet': 'unordered_set',
                        'Vector': 'vector',
                        'LwwRegister': 'lww_register',
                        'Counter': 'counter',
                        'Rga': 'rga'
                    };
                    const readableType = typeMap[typeInfo] || typeInfo.toLowerCase();
                    if (counterValue !== null) {
                        labelText = `${fieldName} (${readableType}) = ${counterValue}`;
                    } else {
                        labelText = `${fieldName} (${readableType})`;
                    }
                } else {
                    if (counterValue !== null) {
                        labelText = `${fieldName} = ${counterValue}`;
                    } else {
                        labelText = fieldName;
                    }
                }
            }
            // For Entry types, show key: value format
            else if (data.type === 'Entry') {
                if (data.data) {
                    const stateData = data.data;
                    let keyStr = '';
                    let valueStr = '';
                    
                    // Get key
                    if (stateData.key && stateData.key.parsed !== undefined) {
                        keyStr = JSON.stringify(stateData.key.parsed, null, 0);
                    } else if (stateData.key) {
                        keyStr = String(stateData.key);
                    }
                    
                    // Get value
                    if (stateData.value && stateData.value.parsed !== undefined) {
                        valueStr = JSON.stringify(stateData.value.parsed, null, 0);
                    } else if (stateData.value) {
                        valueStr = String(stateData.value);
                    }
                    
                    // Format as "key: value"
                    if (keyStr && valueStr) {
                        labelText = `${keyStr}: ${valueStr}`;
                    } else if (keyStr) {
                        labelText = `Key: ${keyStr}`;
                    } else if (valueStr) {
                        labelText = valueStr;
                    } else {
                        labelText = data.id ? `${data.id.substring(0, 8)}...` : 'Entry';
                    }
                } else {
                    labelText = data.id ? `${data.id.substring(0, 8)}...` : 'Entry';
                }
            }
            // For Root
            else if (data.type === 'Root') {
                labelText = 'Root';
            }
            // Fallback
            else {
                labelText = data.id ? `${data.id.substring(0, 8)}...` : 'Node';
            }
            
            // Add deleted indicator before the text if deleted
            if (isDeleted) {
                text.append('tspan')
                    .attr('x', (!d.children && !d._children) ? 8 : 0)
                    .attr('dy', '0')
                    .attr('fill', '#f44336')
                    .text('🗑️ ');
            }
            
            // Split long text into lines (wrap at ~80 characters)
            const maxWidth = 600; // Approximate max width in pixels
            const words = labelText.split(/(\s+)/);
            let line = '';
            const startX = (!d.children && !d._children) ? (isDeleted ? 20 : 8) : (isDeleted ? 12 : 0);
            let tspan = text.append('tspan')
                .attr('x', startX)
                .attr('dy', '0');
            
            words.forEach(word => {
                const testLine = line + word;
                // Rough estimate: 6 pixels per character
                if (testLine.length * 6 > maxWidth && line.length > 0) {
                    tspan.text(line);
                    line = word;
                    tspan = text.append('tspan')
                        .attr('x', startX)
                        .attr('dy', '14'); // Line height
                } else {
                    line = testLine;
                }
            });
            tspan.text(line);
            
            // Add strikethrough line for deleted items
            if (isDeleted && labelText) {
                // Estimate text width (rough approximation)
                const textWidth = labelText.length * 6; // ~6 pixels per character
                const textX = startX;
                const textY = nodeHeight / 2;
                const lineY = textY - 2; // Slightly above center for strikethrough
                
                g.append('line')
                    .attr('x1', textX)
                    .attr('y1', lineY)
                    .attr('x2', textX + Math.min(textWidth, maxWidth))
                    .attr('y2', lineY)
                    .attr('stroke', '#f44336')
                    .attr('stroke-width', 1)
                    .attr('opacity', 0.7);
            }
        });

        // Add expand/collapse indicator for folders (clickable)
        const expandableNodes = nodesEnter.filter(d => d.children || d._children);
        expandableNodes.insert('text', ':first-child')
            .attr('x', -8)
            .attr('y', d => (d._height || baseNodeHeight) / 2)
            .attr('dy', '0.35em')
            .attr('font-size', '10px')
            .attr('fill', '#999')
            .style('cursor', 'pointer')
            .text(d => d.children ? '▼' : '▶');

        // Add hover effect (same as tree view)
        nodesEnter
            .on('mouseover', function(event, d) {
                // Highlight circle if present
                const circle = d3.select(this).select('circle');
                if (!circle.empty()) {
                    circle.attr('stroke-width', 2).attr('stroke', '#fff');
                }
                // Highlight all text elements
                d3.select(this).selectAll('text').attr('fill', '#fff');
                // Show tooltip (same as tree view)
                const content = this.formatTooltipContent(d);
                this.tooltipManager.showTooltip(event, content, 'state-tooltip-temp');
            }.bind(this))
            .on('mousemove', (event) => {
                this.tooltipManager.moveTooltip(event);
            })
            .on('mouseout', function(event, d) {
                const data = d.data;
                const isDeleted = data.deleted_at !== null && data.deleted_at !== undefined;
                
                // Reset circle if present
                const circle = d3.select(this).select('circle');
                if (!circle.empty()) {
                    circle.attr('stroke-width', 1).attr('stroke', '#666');
                }
                // Reset all text elements
                d3.select(this).selectAll('text').each(function() {
                    const textEl = d3.select(this);
                    const textContent = textEl.text();
                    const textNodes = d3.select(this.parentNode).selectAll('text').nodes();
                    const isFirst = this === textNodes[0];
                    
                    if (textContent.includes('🗑️')) {
                        textEl.attr('fill', '#f44336'); // Keep deleted icon red
                    } else if (isFirst) {
                        textEl.attr('fill', '#999'); // Expand/collapse indicator
                    } else {
                        textEl.attr('fill', isDeleted ? '#888' : '#d4d4d4'); // Grayed for deleted, normal for others
                    }
                });
                this.tooltipManager.hideTooltip();
            }.bind(this))
            .on('click', function(event, d) {
                event.stopPropagation();
                // Toggle expand/collapse
                if (d.children) {
                    d._children = d.children;
                    d.children = null;
                } else if (d._children) {
                    d.children = d._children;
                    d._children = null;
                }
                // Re-render to show updated state
                this.renderFolder();
            }.bind(this));
        
        // Make expand/collapse indicators clickable too
        expandableNodes.select('text')
            .on('click', function(event, d) {
                event.stopPropagation();
                // Toggle expand/collapse
                if (d.children) {
                    d._children = d.children;
                    d.children = null;
                } else if (d._children) {
                    d.children = d._children;
                    d._children = null;
                }
                // Re-render to show updated state
                this.renderFolder();
            }.bind(this));

        // Setup zoom
        const zoom = d3.zoom()
            .scaleExtent([0.1, 3])
            .on('zoom', (event) => {
                g.attr('transform', `translate(${event.transform.x + 20},${event.transform.y + 20}) scale(${event.transform.k})`);
            });

        svg.call(zoom);

        // Store zoom for reset
        this.currentZoom = zoom;
    }
}
