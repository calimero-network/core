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
     * Load and process DAG data from JSON
     */
    async load() {
        if (this.state.dagData) {
            this.render();
            return;
        }

        const data = this.state.jsonData;
        if (!data || !data.Meta || !data.Delta) {
            throw new Error('No Meta or Delta data found in database export');
        }

        // Extract contexts and build node list
        const contexts = Object.keys(data.Delta);
        const nodes = [];
        const links = [];

        // Create nodes from deltas
        contexts.forEach(contextId => {
            const deltas = data.Delta[contextId] || [];
            deltas.forEach((delta, idx) => {
                nodes.push({
                    id: `${contextId}-${idx}`,
                    context: contextId,
                    delta: delta,
                    index: idx
                });
            });
        });

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

        // Simple grid layout for now
        this.state.dagData.nodes.forEach((node, i) => {
            const x = (i % 10) * 80 + 50;
            const y = Math.floor(i / 10) * 80 + 50;

            const nodeGroup = g.append('g')
                .attr('class', 'dag-node')
                .attr('transform', `translate(${x},${y})`);

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
                .text(node.id.substring(0, 8));
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

        // D3 force simulation
        const simulation = d3.forceSimulation(this.state.dagData.nodes)
            .force('charge', d3.forceManyBody().strength(-300))
            .force('center', d3.forceCenter(width / 2, height / 2))
            .force('collision', d3.forceCollide().radius(30));

        const nodes = g.selectAll('circle')
            .data(this.state.dagData.nodes)
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
            nodes
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

        const { nodes, contexts } = this.state.dagData;

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
