/**
 * DAG Visualizer Module
 * Renders Directed Acyclic Graph using D3.js
 * @module dag-visualizer
 */

import { UIManager } from './ui-manager.js';

export class DAGVisualizer {
    /**
     * @param {import('./app-state.js').AppState} state - Application state
     */
    constructor(state) {
        this.state = state;
        this.currentZoom = null;
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

        select.innerHTML = '<option value="all">All Contexts</option>';
        contexts.forEach(context => {
            const option = document.createElement('option');
            option.value = context;
            option.textContent = context.substring(0, 8) + '...';
            select.appendChild(option);
        });
    }

    /**
     * Get filtered nodes and links based on selected context
     * @returns {{nodes: Array, links: Array}}
     */
    getFilteredData() {
        const contextSelect = document.getElementById('context-select');
        const selectedContext = contextSelect?.value || 'all';

        if (selectedContext === 'all') {
            return {
                nodes: this.state.dagData.nodes,
                links: this.state.dagData.links
            };
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
        const svg = d3.select('#dag-svg');
        svg.selectAll('*').remove();

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
                .attr('transform', `translate(${pos.x},${pos.y})`);

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
        const svg = d3.select('#dag-svg');
        svg.selectAll('*').remove();

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
            d3.select('#dag-svg')
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
        const svg = document.getElementById('dag-svg');
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
}
