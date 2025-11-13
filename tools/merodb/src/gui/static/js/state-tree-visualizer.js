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
     */
    constructor(state) {
        this.state = state;
        this.currentZoom = null;
        this.root = null;
        this.updateFn = null;
        this.tooltipManager = new TooltipManager();
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
     */
    async load() {
        if (this.state.stateTreeData) {
            this.render();
            return;
        }

        if (!this.state.currentWasmFile) {
            throw new Error('WASM file is required for state tree visualization');
        }

        UIManager.showElement('state-loading');
        UIManager.hideElement('state-error');

        try {
            const response = await ApiService.loadStateTree(
                this.state.currentDbPath,
                this.state.currentWasmFile
            );

            this.state.stateTreeData = response.data;
            this.populateContextSelector();
            this.render();
        } catch (error) {
            throw error;
        } finally {
            UIManager.hideElement('state-loading');
        }
    }

    /**
     * Populate context selector dropdown with available contexts
     */
    populateContextSelector() {
        const select = document.getElementById('state-context-select');
        if (!select || !this.state.stateTreeData?.contexts) return;

        // Clear existing options
        select.innerHTML = '';

        // Add option for each context
        this.state.stateTreeData.contexts.forEach((context, index) => {
            const option = document.createElement('option');
            option.value = index.toString();
            option.textContent = `Context ${index + 1}: ${context.context_id.substring(0, 8)}...`;
            select.appendChild(option);
        });

        // Select the first context by default
        if (this.state.stateTreeData.contexts.length > 0) {
            select.value = '0';
        }
    }

    /**
     * Get the tree data for the currently selected context
     * @returns {Object|null} Tree data for d3.hierarchy
     */
    getSelectedContextTree() {
        if (!this.state.stateTreeData?.contexts) return null;

        const select = document.getElementById('state-context-select');
        const selectedValue = select?.value || '0';

        const index = parseInt(selectedValue, 10);
        const tree = this.state.stateTreeData.contexts[index]?.tree;

        // Deep clone to prevent mutation of original stateTreeData
        if (!tree) return null;

        try {
            return JSON.parse(JSON.stringify(tree));
        } catch (error) {
            console.error(`Failed to clone tree for context ${index}:`, error);
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

        const layout = document.getElementById('state-layout-select')?.value || 'tree';

        if (layout === 'tree') {
            this.renderTree();
        } else {
            this.renderRadial();
        }

        this.updateStats();
    }

    /**
     * Render top-down tree layout with collapsible nodes
     */
    renderTree() {
        const svg = d3.select('#state-svg');
        svg.selectAll('*').remove();

        const width = svg.node().getBoundingClientRect().width;
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
     * Render radial tree layout
     */
    renderRadial() {
        const svg = d3.select('#state-svg');
        svg.selectAll('*').remove();

        const width = svg.node().getBoundingClientRect().width;
        const height = 600;
        const radius = Math.min(width, height) / 2 - 50;

        svg.attr('viewBox', [0, 0, width, height]);

        const g = svg.append('g')
            .attr('transform', `translate(${width / 2},${height / 2})`);

        const hierarchyData = this.getSelectedContextTree();
        if (!hierarchyData) {
            console.error('No tree data available for selected context');
            return;
        }
        const root = d3.hierarchy(hierarchyData);

        const treeLayout = d3.tree()
            .size([2 * Math.PI, radius])
            .separation((a, b) => (a.parent == b.parent ? 1 : 2) / a.depth);

        treeLayout(root);

        // Draw links
        g.selectAll('.state-link')
            .data(root.links())
            .enter().append('path')
            .attr('class', 'state-link')
            .attr('d', d3.linkRadial()
                .angle(d => d.x)
                .radius(d => d.y));

        // Draw nodes
        const nodes = g.selectAll('.state-node')
            .data(root.descendants())
            .enter().append('g')
            .attr('class', 'state-node')
            .attr('transform', d => `rotate(${d.x * 180 / Math.PI - 90}) translate(${d.y},0)`);

        nodes.append('circle')
            .attr('r', 4)
            .attr('class', d => {
                if (this.hasDecodedData(d)) return 'has-data';
                if (!d.children) return 'leaf';
                return '';
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

        // Add node ID labels for radial layout
        nodes.append('text')
            .attr('dy', '0.31em')
            .attr('x', d => d.x < Math.PI === !d.children ? 6 : -6)
            .attr('text-anchor', d => d.x < Math.PI === !d.children ? 'start' : 'end')
            .attr('transform', d => d.x >= Math.PI ? 'rotate(180)' : null)
            .text(d => {
                const id = d.data.id || 'N/A';
                return id !== 'N/A' ? `${id.substring(0, 8)}...` : 'N/A';
            })
            .style('font-size', '10px')
            .style('fill', '#bbb')
            .style('pointer-events', 'none');

        this.root = root;
        this.setupZoom(svg, g);
    }

    /**
     * Setup D3 zoom behavior
     * @param {d3.Selection} svg - SVG element
     * @param {d3.Selection} g - Group element
     */
    setupZoom(svg, g) {
        const zoom = d3.zoom()
            .scaleExtent([0.1, 10])
            .on('zoom', (event) => {
                g.attr('transform', event.transform);
            });

        svg.call(zoom);
        this.currentZoom = zoom;
    }

    /**
     * Reset zoom to identity
     */
    resetZoom() {
        if (this.currentZoom) {
            d3.select('#state-svg')
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
        const container = document.getElementById('state-stats');
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
        const svg = document.getElementById('state-svg');
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
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Value:</span>`;
                html += `  <span class="visualization__tooltip-value"><pre>${JSON.stringify(stateData.value.parsed, null, 2)}</pre></span>`;
                html += `</div>`;
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
}
