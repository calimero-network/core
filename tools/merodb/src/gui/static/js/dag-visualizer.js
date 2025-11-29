/**
 * DAG Visualizer Module
 * Renders Directed Acyclic Graph using D3.js
 * @module dag-visualizer
 */

import { UIManager } from './ui-manager.js';
import { TooltipManager } from './tooltip-manager.js';

export class DAGVisualizer {
    /**
     * @param {import('./app-state.js').AppState} state - Application state
     * @param {string} svgId - Optional SVG element ID (defaults to 'dag-svg')
     */
    constructor(state, svgId = 'dag-svg') {
        this.state = state;
        this.currentZoom = null;
        this.tooltipManager = new TooltipManager();
        this.svgId = svgId;
    }

    /**
     * Load and process DAG data using the /api/dag endpoint
     */
    async load() {
        if (this.state.dagData) {
            this.render();
            return;
        }

        // Import ApiService
        const { ApiService } = await import('./api-service.js');

        // Get the database path from app state
        const dbPath = this.state.currentDbPath;
        if (!dbPath) {
            throw new Error('Database path is required');
        }

        // Load DAG data from the API
        const dagData = await ApiService.loadDAG(dbPath);

        // Extract nodes and edges from the API response
        const nodes = dagData.nodes || [];
        const edges = dagData.edges || [];

        // Build links from edges (API returns edges with source/target format)
        const links = edges.map(edge => ({
            source: edge.source,
            target: edge.target
        }));

        // Extract unique contexts from nodes
        const contextsSet = new Set();
        nodes.forEach(node => {
            if (node.context_id) {
                contextsSet.add(node.context_id);
            }
        });
        const contexts = Array.from(contextsSet);

        // Store processed data
        this.state.dagData = { nodes, links, contexts };

        // Populate context selector
        this.populateContextSelector(contexts);

        // Render visualization
        this.render();
    }


    /**
     * Populate the context dropdown
     * @param {string[]} contexts - List of context IDs
     */
    populateContextSelector(contexts) {
        const select = document.getElementById('context-select');
        if (!select) return;

        select.innerHTML = '';
        contexts.forEach(context => {
            const option = document.createElement('option');
            option.value = context;
            option.textContent = context.substring(0, 8) + '...';
            select.appendChild(option);
        });

        // Select the first context by default
        if (contexts.length > 0) {
            select.value = contexts[0];
        }
    }

    /**
     * Get filtered nodes and links based on selected context
     * @returns {{nodes: Array, links: Array}}
     */
    getFilteredData() {
        const contextSelect = document.getElementById('context-select');
        const selectedContext = contextSelect?.value;

        if (!selectedContext) {
            return { nodes: [], links: [] };
        }

        // Filter nodes for the selected context
        const filteredNodes = this.state.dagData.nodes.filter(node =>
            node.context_id === selectedContext
        );

        // Create a set of node IDs for quick lookup
        const nodeIds = new Set(filteredNodes.map(node => node.id));

        // Filter links to only include those between filtered nodes
        const filteredLinks = this.state.dagData.links.filter(link =>
            nodeIds.has(link.source) && nodeIds.has(link.target)
        );

        return { nodes: filteredNodes, links: filteredLinks };
    }

    /**
     * Render the DAG based on selected layout
     */
    render() {
        // Clean up existing tooltips before re-render to prevent memory leaks
        this.tooltipManager.cleanupTooltips();

        const layout = document.getElementById('layout-select')?.value || 'hierarchical';

        if (layout === 'hierarchical') {
            this.renderHierarchical();
        } else {
            this.renderForce();
        }

        this.updateStats();
    }

    /**
     * Render hierarchical tree layout
     */
    renderHierarchical() {
        const svg = d3.select(`#${this.svgId}`);
        svg.selectAll('*').remove();

        if (!svg.node()) {
            console.error(`[DAGVisualizer.renderHierarchical] SVG element #${this.svgId} not found`);
            return;
        }

        const width = svg.node().getBoundingClientRect().width;
        const height = 500;
        svg.attr('viewBox', [0, 0, width, height]);

        const g = svg.append('g');

        // Get filtered data based on selected context
        const { nodes, links } = this.getFilteredData();

        // Create a node position map for drawing links
        const nodePositions = new Map();
        nodes.forEach((node, i) => {
            const x = (i % 10) * 80 + 50;
            const y = Math.floor(i / 10) * 80 + 50;
            nodePositions.set(node.id, { x, y });
        });

        // Draw links (arrows) first so they appear behind nodes
        const linksGroup = g.append('g').attr('class', 'links');

        // Define arrow marker
        svg.append('defs').append('marker')
            .attr('id', 'arrowhead')
            .attr('viewBox', '0 -5 10 10')
            .attr('refX', 25)  // Position at edge of target node (20 radius + 5 buffer)
            .attr('refY', 0)
            .attr('markerWidth', 6)
            .attr('markerHeight', 6)
            .attr('orient', 'auto')
            .append('path')
            .attr('d', 'M0,-5L10,0L0,5')
            .attr('fill', '#666');

        links.forEach(link => {
            const source = nodePositions.get(link.source);
            const target = nodePositions.get(link.target);

            if (source && target) {
                linksGroup.append('line')
                    .attr('x1', source.x)
                    .attr('y1', source.y)
                    .attr('x2', target.x)
                    .attr('y2', target.y)
                    .attr('stroke', '#666')
                    .attr('stroke-width', 2)
                    .attr('marker-end', 'url(#arrowhead)');
            }
        });

        // Draw nodes
        nodes.forEach((node, i) => {
            const pos = nodePositions.get(node.id);

            const nodeGroup = g.append('g')
                .attr('class', 'dag-node')
                .attr('transform', `translate(${pos.x},${pos.y})`)
                .style('cursor', 'pointer')
                .on('mouseover', async (event) => {
                    const nodeId = node.delta_id || node.id;

                    // Show initial tooltip with basic info
                    const content = this.formatTooltipContent(node);
                    this.tooltipManager.showTooltip(event, content, 'state-tooltip-temp', nodeId);

                    // Load detailed info on demand (actions and events)
                    await this.loadAndUpdateTooltip(event, node);
                })
                .on('mousemove', (event) => {
                    this.tooltipManager.moveTooltip(event);
                })
                .on('mouseout', () => {
                    this.tooltipManager.hideTooltip();
                });

            nodeGroup.append('circle')
                .attr('r', 20)
                .attr('fill', '#0e639c')
                .attr('stroke', '#007acc')
                .attr('stroke-width', 2);

            nodeGroup.append('text')
                .attr('text-anchor', 'middle')
                .attr('dy', 4)
                .attr('fill', '#d4d4d4')
                .attr('font-size', '10px')
                .text((node.delta_id || node.id).substring(0, 8));
        });

        this.setupZoom(svg, g);
    }

    /**
     * Render force-directed layout
     */
    renderForce() {
        const svg = d3.select(`#${this.svgId}`);
        svg.selectAll('*').remove();

        if (!svg.node()) {
            console.error(`[DAGVisualizer.renderForce] SVG element #${this.svgId} not found`);
            return;
        }

        const width = svg.node().getBoundingClientRect().width;
        const height = 500;
        svg.attr('viewBox', [0, 0, width, height]);

        const g = svg.append('g');

        // Get filtered data based on selected context
        const { nodes, links } = this.getFilteredData();

        // Define arrow marker
        svg.append('defs').append('marker')
            .attr('id', 'arrowhead-force')
            .attr('viewBox', '0 -5 10 10')
            .attr('refX', 25)
            .attr('refY', 0)
            .attr('markerWidth', 6)
            .attr('markerHeight', 6)
            .attr('orient', 'auto')
            .append('path')
            .attr('d', 'M0,-5L10,0L0,5')
            .attr('fill', '#666');

        // Draw links first
        const linkElements = g.append('g')
            .attr('class', 'links')
            .selectAll('line')
            .data(links)
            .enter().append('line')
            .attr('stroke', '#666')
            .attr('stroke-width', 2)
            .attr('marker-end', 'url(#arrowhead-force)');

        // D3 force simulation
        const simulation = d3.forceSimulation(nodes)
            .force('link', d3.forceLink(links).id(d => d.id).distance(100))
            .force('charge', d3.forceManyBody().strength(-300))
            .force('center', d3.forceCenter(width / 2, height / 2))
            .force('collision', d3.forceCollide().radius(30));

        const nodeElements = g.selectAll('circle')
            .data(nodes)
            .enter().append('circle')
            .attr('r', 20)
            .attr('fill', '#0e639c')
            .attr('stroke', '#007acc')
            .attr('stroke-width', 2)
            .attr('class', 'dag-node')
            .style('cursor', 'pointer')
            .on('mouseover', async (event, d) => {
                const nodeId = d.delta_id || d.id;

                // Show initial tooltip with basic info
                const content = this.formatTooltipContent(d);
                this.tooltipManager.showTooltip(event, content, 'state-tooltip-temp', nodeId);

                // Load detailed info on demand (actions and events)
                await this.loadAndUpdateTooltip(event, d);
            })
            .on('mousemove', (event) => {
                this.tooltipManager.moveTooltip(event);
            })
            .on('mouseout', () => {
                this.tooltipManager.hideTooltip();
            })
            .call(d3.drag()
                .on('start', dragStarted)
                .on('drag', dragged)
                .on('end', dragEnded));

        simulation.on('tick', () => {
            linkElements
                .attr('x1', d => d.source.x)
                .attr('y1', d => d.source.y)
                .attr('x2', d => d.target.x)
                .attr('y2', d => d.target.y);

            nodeElements
                .attr('cx', d => d.x)
                .attr('cy', d => d.y);
        });

        function dragStarted(event) {
            if (!event.active) simulation.alphaTarget(0.3).restart();
            event.subject.fx = event.subject.x;
            event.subject.fy = event.subject.y;
        }

        function dragged(event) {
            event.subject.fx = event.x;
            event.subject.fy = event.y;
        }

        function dragEnded(event) {
            if (!event.active) simulation.alphaTarget(0);
            event.subject.fx = null;
            event.subject.fy = null;
        }

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
            const svgId = this.svgId || 'dag-svg';
            d3.select(`#${svgId}`)
                .transition()
                .duration(750)
                .call(this.currentZoom.transform, d3.zoomIdentity);
        }
    }

    /**
     * Update statistics display
     */
    updateStats() {
        const container = document.getElementById('dag-stats');
        if (!container || !this.state.dagData) return;

        // Get filtered data for accurate stats
        const { nodes } = this.getFilteredData();
        const { contexts } = this.state.dagData;

        container.innerHTML = `
            <div class="stats__item">
                <div class="stats__label">Nodes</div>
                <div class="stats__value">${nodes.length}</div>
            </div>
            <div class="stats__item">
                <div class="stats__label">Contexts</div>
                <div class="stats__value">${contexts.length}</div>
            </div>
        `;
    }

    /**
     * Export DAG as SVG file
     */
    exportImage() {
        const svgId = this.svgId || 'dag-svg';
        const svg = document.getElementById(svgId);
        if (!svg) return;

        const serializer = new XMLSerializer();
        const source = serializer.serializeToString(svg);
        const blob = new Blob([source], { type: 'image/svg+xml;charset=utf-8' });
        const url = URL.createObjectURL(blob);

        const link = document.createElement('a');
        link.href = url;
        link.download = 'dag-visualization.svg';
        link.click();

        URL.revokeObjectURL(url);
    }

    /**
     * Format tooltip content HTML
     * @param {Object} node - DAG node object
     * @returns {string} HTML content
     */
    formatTooltipContent(node) {
        let html = '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Node Information</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Delta ID:</span>`;
        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(node.delta_id || node.id, 'Delta ID')}</span>`;
        html += `</div>`;
        html += `<div class="visualization__tooltip-row">`;
        html += `  <span class="visualization__tooltip-label">Context:</span>`;
        html += `  <span class="visualization__tooltip-value">${node.context_id ? TooltipManager.formatHash(node.context_id, 'Context') : 'N/A'}</span>`;
        html += `</div>`;

        if (node.is_genesis !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Genesis Node:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.is_genesis ? 'Yes' : 'No'}</span>`;
            html += `</div>`;
        }

        if (node.is_dag_head !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">DAG Head:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.is_dag_head ? 'Yes' : 'No'}</span>`;
            html += `</div>`;
        }

        if (node.applied !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Applied:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.applied ? 'Yes' : 'No'}</span>`;
            html += `</div>`;
        }

        if (node.actions_size !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Actions Size:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.actions_size} bytes</span>`;
            html += `</div>`;
        }

        if (node.expected_root_hash !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Expected Root Hash:</span>`;
            html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(node.expected_root_hash, 'Expected Root Hash')}</span>`;
            html += `</div>`;
        }

        html += `</div>`;

        // Actions section (deserialized)
        if (node.actions && Array.isArray(node.actions) && node.actions.length > 0) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Actions (${node.actions.length})</div>`;
            node.actions.forEach((action, idx) => {
                html += `<div class="visualization__tooltip-subsection">`;
                html += `<div class="visualization__tooltip-subtitle">Action ${idx + 1}: ${action.type}</div>`;

                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Entity ID:</span>`;
                html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(action.id, `Action-${idx}-ID`)}</span>`;
                html += `</div>`;

                if (action.type === 'Add' || action.type === 'Update') {
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Data Size:</span>`;
                    html += `  <span class="visualization__tooltip-value">${action.data_size} bytes</span>`;
                    html += `</div>`;

                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Ancestors:</span>`;
                    html += `  <span class="visualization__tooltip-value">${action.ancestors_count}</span>`;
                    html += `</div>`;

                    if (action.metadata) {
                        html += `<div class="visualization__tooltip-row">`;
                        html += `  <span class="visualization__tooltip-label">Created At:</span>`;
                        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(action.metadata.created_at)}</span>`;
                        html += `</div>`;

                        html += `<div class="visualization__tooltip-row">`;
                        html += `  <span class="visualization__tooltip-label">Updated At:</span>`;
                        html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(action.metadata.updated_at)}</span>`;
                        html += `</div>`;
                    }
                } else if (action.type === 'DeleteRef') {
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Deleted At:</span>`;
                    html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(action.deleted_at)}</span>`;
                    html += `</div>`;
                }

                html += `</div>`;
            });
            html += `</div>`;
        }

        // Events section (deserialized)
        if (node.events && Array.isArray(node.events) && node.events.length > 0) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Events (${node.events.length})</div>`;
            node.events.forEach((event, idx) => {
                html += `<div class="visualization__tooltip-subsection">`;
                html += `<div class="visualization__tooltip-subtitle">Event ${idx + 1}</div>`;

                if (event.kind) {
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Kind:</span>`;
                    html += `  <span class="visualization__tooltip-value">${event.kind}</span>`;
                    html += `</div>`;
                }

                if (event.handler) {
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Handler:</span>`;
                    html += `  <span class="visualization__tooltip-value">${event.handler}</span>`;
                    html += `</div>`;
                }

                if (event.data) {
                    const dataSize = Array.isArray(event.data) ? event.data.length : JSON.stringify(event.data).length;
                    html += `<div class="visualization__tooltip-row">`;
                    html += `  <span class="visualization__tooltip-label">Data Size:</span>`;
                    html += `  <span class="visualization__tooltip-value">${dataSize} bytes</span>`;
                    html += `</div>`;
                }

                html += `</div>`;
            });
            html += `</div>`;
        }

        // Hybrid Logical Clock section
        if (node.hlc || node.logical_counter !== undefined || node.physical_time !== undefined) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Hybrid Logical Clock</div>`;

            if (node.hlc) {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">HLC:</span>`;
                html += `  <span class="visualization__tooltip-value">${node.hlc}</span>`;
                html += `</div>`;
            }

            if (node.physical_time !== undefined) {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Physical Time:</span>`;
                html += `  <span class="visualization__tooltip-value">${node.physical_time}</span>`;
                html += `</div>`;
            }

            if (node.logical_counter !== undefined) {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Logical Counter:</span>`;
                html += `  <span class="visualization__tooltip-value">${node.logical_counter}</span>`;
                html += `</div>`;
            }

            html += `</div>`;
        }

        // Parent relationships section
        if (node.parents && node.parents.length > 0) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Parents (${node.parent_count || node.parents.length})</div>`;
            node.parents.forEach((prevId, idx) => {
                html += `<div class="visualization__tooltip-row">`;
                html += `  <span class="visualization__tooltip-label">Parent ${idx + 1}:</span>`;
                html += `  <span class="visualization__tooltip-value">${TooltipManager.formatHash(prevId, `Parent-${idx}`)}</span>`;
                html += `</div>`;
            });
            html += `</div>`;
        } else if (node.parent_count !== undefined) {
            html += '<div class="visualization__tooltip-section">';
            html += `<div class="visualization__tooltip-title">Parents</div>`;
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Parent Count:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.parent_count}</span>`;
            html += `</div>`;
            html += `</div>`;
        }

        // Timestamps section
        html += '<div class="visualization__tooltip-section">';
        html += `<div class="visualization__tooltip-title">Timestamps</div>`;

        if (node.timestamp !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Timestamp:</span>`;
            html += `  <span class="visualization__tooltip-value">${node.timestamp === 0 ? '0 (Genesis)' : node.timestamp}</span>`;
            html += `</div>`;
        }

        if (node.created_at !== undefined) {
            html += `<div class="visualization__tooltip-row">`;
            html += `  <span class="visualization__tooltip-label">Created:</span>`;
            html += `  <span class="visualization__tooltip-value">${TooltipManager.formatTimestamp(node.created_at)}</span>`;
            html += `</div>`;
        }

        html += `</div>`;

        return html;
    }

    /**
     * Load delta details on demand and update the tooltip
     * @param {Event} event - Mouse event for positioning
     * @param {Object} node - DAG node object
     */
    async loadAndUpdateTooltip(event, node) {
        // Skip loading details for genesis nodes
        if (node.is_genesis) {
            return;
        }

        // Skip if no context_id or delta_id
        if (!node.context_id || !node.delta_id) {
            return;
        }

        // Skip if details are already loaded
        if (node.actions || node.events) {
            return;
        }

        try {
            // Import ApiService dynamically
            const { ApiService } = await import('./api-service.js');

            // Get the database path from app state
            const dbPath = this.state.currentDbPath;
            if (!dbPath) {
                return;
            }

            // Store the node ID to check for race conditions
            const nodeId = node.delta_id || node.id;

            // Fetch delta details
            const details = await ApiService.loadDeltaDetails(dbPath, node.context_id, node.delta_id);

            // Check if tooltip is still showing the same node (race condition check)
            const currentTooltip = this.tooltipManager.getCurrentTooltipNode();
            if (!currentTooltip || currentTooltip !== nodeId) {
                // User has moved to a different node, don't update
                return;
            }

            // Update node with details
            if (details.actions) {
                node.actions = details.actions;
            }
            if (details.events) {
                node.events = details.events;
            }

            // Re-render tooltip with updated content
            const updatedContent = this.formatTooltipContent(node);
            this.tooltipManager.updateTooltipContent(updatedContent);
        } catch (error) {
            console.error('Failed to load delta details:', error);
            // Don't show error to user, just log it
        }
    }
}
