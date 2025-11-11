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
        this.activeTooltips = [];
        this.tooltipCounter = 0;
        this.tooltipPinned = false;
        this.currentTooltip = null;
        this.pinnedDuringHover = false;
        this.setupKeyboardListeners();
    }

    /**
     * Setup keyboard listeners for tooltip pinning
     */
    setupKeyboardListeners() {
        window.addEventListener('keydown', (e) => {
            if (e.key === 'Control' || e.key === 'Meta' || e.ctrlKey || e.metaKey) {
                this.tooltipPinned = true;
            }
        });

        window.addEventListener('keyup', (e) => {
            if (e.key === 'Control' || e.key === 'Meta') {
                this.tooltipPinned = false;
                // Don't auto-hide on key release, let mouseout handle it
            }
        });
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

        // Select "All Contexts" by default to show all state items
        select.value = 'all';
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
            // Create a virtual root that contains all contexts as children
            const allContextTrees = this.state.stateTreeData.contexts.map((context, index) => ({
                ...context.tree,
                id: `context-${index}-${context.tree.id}`,
                type: 'ContextRoot',
                context_id: context.context_id,
                context_index: index
            }));

            return {
                id: 'all-contexts-root',
                type: 'VirtualRoot',
                children: allContextTrees,
                children_count: allContextTrees.length
            };
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
            .attr('class', d => d.children ? '' : 'leaf')
            .on('mouseover', (event, d) => {
                this.showTooltip(event, d);
            })
            .on('mousemove', (event) => {
                this.moveTooltip(event);
            })
            .on('mouseout', () => {
                this.hideTooltip();
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
     * Show tooltip with node information
     * @param {MouseEvent} event - Mouse event
     * @param {Object} d - Node data
     */
    showTooltip(event, d) {
        // If Ctrl/Cmd is pressed, create a new pinned tooltip
        // Only create one tooltip per node per hover session
        if (this.tooltipPinned) {
            // Check if we already created a tooltip for this node in this hover session
            if (!this.pinnedDuringHover) {
                this.createPinnedTooltip(event, d.data);
                this.pinnedDuringHover = true;
            }
            return;
        }

        // Otherwise, show temporary tooltip
        if (!this.currentTooltip) {
            this.currentTooltip = d3.select('body').append('div')
                .attr('class', 'tooltip state-tooltip-temp')
                .style('position', 'fixed')
                .style('pointer-events', 'none');
        }

        const html = this.formatTooltipContent(d.data);
        this.currentTooltip
            .html(html)
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`)
            .classed('hidden', false);
    }

    /**
     * Format tooltip content HTML
     * @param {Object} data - Node data
     * @returns {string} HTML content
     */
    formatTooltipContent(data) {
        const formatTimestamp = (ts) => {
            if (!ts) return 'N/A';
            const date = new Date(ts / 1000000);
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

        return html;
    }

    /**
     * Create a pinned, draggable tooltip
     * @param {MouseEvent} event - Mouse event
     * @param {Object} data - Node data
     */
    createPinnedTooltip(event, data) {
        const tooltipId = `tooltip-${this.tooltipCounter++}`;

        const tooltip = d3.select('body').append('div')
            .attr('id', tooltipId)
            .attr('class', 'tooltip state-tooltip-pinned')
            .style('position', 'fixed')
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`)
            .style('pointer-events', 'auto')
            .style('cursor', 'move');

        // Add close button
        const closeBtn = tooltip.append('button')
            .attr('class', 'tooltip-close')
            .html('&times;')
            .on('click', () => {
                tooltip.remove();
                this.activeTooltips = this.activeTooltips.filter(t => t.id !== tooltipId);
            });

        // Add content
        tooltip.append('div')
            .attr('class', 'tooltip-content')
            .html(this.formatTooltipContent(data));

        // Make draggable
        this.makeDraggable(tooltip);

        this.activeTooltips.push({ id: tooltipId, element: tooltip });
    }

    /**
     * Make a tooltip draggable
     * @param {d3.Selection} tooltip - Tooltip element
     */
    makeDraggable(tooltip) {
        const tooltipNode = tooltip.node();
        let isDragging = false;
        let startX = 0;
        let startY = 0;
        let offsetX = 0;
        let offsetY = 0;

        const dragStart = (e) => {
            if (e.target.classList.contains('tooltip-close')) return;

            isDragging = true;
            const rect = tooltipNode.getBoundingClientRect();
            offsetX = e.clientX - rect.left;
            offsetY = e.clientY - rect.top;
            tooltipNode.style.cursor = 'grabbing';
        };

        const drag = (e) => {
            if (!isDragging) return;
            e.preventDefault();

            const newLeft = e.clientX - offsetX;
            const newTop = e.clientY - offsetY;

            tooltip
                .style('left', `${newLeft}px`)
                .style('top', `${newTop}px`);
        };

        const dragEnd = () => {
            isDragging = false;
            tooltipNode.style.cursor = 'move';
        };

        tooltip.on('mousedown', dragStart);
        d3.select(document)
            .on('mousemove.drag-' + tooltip.attr('id'), drag)
            .on('mouseup.drag-' + tooltip.attr('id'), dragEnd);
    }

    /**
     * Move tooltip to follow mouse
     * @param {MouseEvent} event - Mouse event
     */
    moveTooltip(event) {
        if (this.currentTooltip && !this.tooltipPinned) {
            this.currentTooltip
                .style('left', `${event.pageX + 10}px`)
                .style('top', `${event.pageY + 10}px`);
        }
    }

    /**
     * Hide tooltip (unless pinned)
     */
    hideTooltip() {
        if (!this.tooltipPinned && this.currentTooltip) {
            this.currentTooltip.classed('hidden', true);
        }
        // Reset the flag when mouse leaves the node
        this.pinnedDuringHover = false;
    }
}
