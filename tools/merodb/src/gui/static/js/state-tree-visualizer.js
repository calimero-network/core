/**
 * State Tree Visualizer Module
 * Renders hierarchical state tree using D3.js
 * @module state-tree-visualizer
 */

import { ApiService } from './api-service.js';
import { UIManager } from './ui-manager.js';

export class StateTreeVisualizer {
    /**
     * @param {import('./app-state.js').AppState} state - Application state
     */
    constructor(state) {
        this.state = state;
        this.currentZoom = null;
        this.root = null;
        this.updateFn = null;
        this.tooltip = d3.select('#state-tooltip');
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

        // Clear existing options except "All Contexts"
        select.innerHTML = '<option value="all">All Contexts</option>';

        // Add option for each context
        this.state.stateTreeData.contexts.forEach((context, index) => {
            const option = document.createElement('option');
            option.value = index.toString();
            option.textContent = `Context ${index + 1}: ${context.context_id.substring(0, 8)}...`;
            select.appendChild(option);
        });

        // Select first context by default
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

        if (selectedValue === 'all') {
            // For "all contexts", use the first context
            // TODO: Could merge all contexts or show them separately
            return this.state.stateTreeData.contexts[0]?.tree || null;
        }

        const index = parseInt(selectedValue, 10);
        return this.state.stateTreeData.contexts[index]?.tree || null;
    }

    /**
     * Render tree based on selected layout
     */
    render() {
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
                    if (d.children) {
                        d._children = d.children;
                        d.children = null;
                    } else if (d._children) {
                        d.children = d._children;
                        d._children = null;
                    }
                    update(d);
                    this.updateStats();
                })
                .on('mouseover', (event, d) => {
                    this.showTooltip(event, d);
                })
                .on('mousemove', (event) => {
                    this.moveTooltip(event);
                })
                .on('mouseout', () => {
                    this.hideTooltip();
                });

            nodeEnter.append('circle')
                .attr('r', 6)
                .attr('class', d => (!d.children && !d._children) ? 'leaf' : '');

            // Transition nodes to their new position
            const nodeUpdate = nodeEnter.merge(node);

            nodeUpdate.transition()
                .duration(duration)
                .attr('transform', d => `translate(${d.y},${d.x})`);

            nodeUpdate.select('circle')
                .attr('class', d => {
                    if (!d.children && !d._children) return 'leaf';
                    if (d._children) return 'collapsed';
                    return '';
                });

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
            .attr('class', d => d.children ? '' : 'leaf');

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

        this.root.descendants().forEach(d => {
            if (d._children) {
                d.children = d._children;
                d._children = null;
            }
        });

        if (this.updateFn) {
            this.updateFn(this.root);
        } else {
            this.render();
        }
        this.updateStats();
    }

    /**
     * Collapse entire tree to root's children
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

        // Keep root's immediate children visible
        if (this.root.children) {
            this.root.children.forEach(child => {
                if (child.children) {
                    child._children = child.children;
                    child._children.forEach(collapseNode);
                    child.children = null;
                }
            });
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
     * Show tooltip with node information
     * @param {MouseEvent} event - Mouse event
     * @param {Object} d - Node data
     */
    showTooltip(event, d) {
        const data = d.data;

        const formatTimestamp = (ts) => {
            if (!ts) return 'N/A';
            const date = new Date(ts / 1000000); // Convert nanoseconds to milliseconds
            return date.toLocaleString();
        };

        const formatHash = (hash) => {
            if (!hash) return 'N/A';
            return `${hash.substring(0, 8)}...${hash.substring(hash.length - 8)}`;
        };

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

        html += '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Hashes</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">ID:</span>`;
        html += `  <span class="visualization__tooltip-value">${formatHash(data.id)}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Full Hash:</span>`;
        html += `  <span class="visualization__tooltip-value">${formatHash(data.full_hash)}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Own Hash:</span>`;
        html += `  <span class="visualization__tooltip-value">${formatHash(data.own_hash)}</span>`;
        html += `</div>`;
        if (data.parent_id) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Parent ID:</span>`;
            html += `  <span class="visualization__tooltip-value">${formatHash(data.parent_id)}</span>`;
            html += `</div>`;
        }
        html += `</div>`;

        html += '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Timestamps</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Created:</span>`;
        html += `  <span class="visualization__tooltip-value">${formatTimestamp(data.created_at)}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Updated:</span>`;
        html += `  <span class="visualization__tooltip-value">${formatTimestamp(data.updated_at)}</span>`;
        html += `</div>`;
        if (data.deleted_at) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Deleted:</span>`;
            html += `  <span class="visualization__tooltip-value">${formatTimestamp(data.deleted_at)}</span>`;
            html += `</div>`;
        }
        html += `</div>`;

        this.tooltip
            .html(html)
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`)
            .classed('hidden', false);
    }

    /**
     * Move tooltip to follow mouse
     * @param {MouseEvent} event - Mouse event
     */
    moveTooltip(event) {
        this.tooltip
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`);
    }

    /**
     * Hide tooltip
     */
    hideTooltip() {
        this.tooltip.classed('hidden', true);
    }
}
