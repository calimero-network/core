/**
 * Tooltip Manager Module
 * Shared tooltip functionality for visualizations
 * @module tooltip-manager
 */

export class TooltipManager {
    constructor() {
        this.activeTooltips = [];
        this.tooltipCounter = 0;
        this.tooltipPinned = false;
        this.currentTooltip = null;
        this.currentNodeId = null;  // Track which node the tooltip is currently showing
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
            }
        });

        // Additional handler to catch cases where key is released while another modifier is held
        window.addEventListener('keyup', (e) => {
            if (!e.ctrlKey && !e.metaKey) {
                this.tooltipPinned = false;
            }
        });
    }

    /**
     * Clean up all active tooltips
     */
    cleanupTooltips() {
        this.activeTooltips.forEach(tooltip => {
            // tooltip.element is a D3 selection, not a DOM node
            if (tooltip.element) {
                // Remove global event listeners for this tooltip
                const tooltipId = tooltip.element.attr('id');
                if (tooltipId) {
                    d3.select(document)
                        .on('mousemove.drag-' + tooltipId, null)
                        .on('mouseup.drag-' + tooltipId, null);
                }
                tooltip.element.remove();
            }
        });
        this.activeTooltips = [];

        // Remove the DOM element from currentTooltip
        if (this.currentTooltip) {
            this.currentTooltip.remove();
        }
        this.currentTooltip = null;
    }

    /**
     * Show tooltip with content
     * @param {MouseEvent} event - Mouse event
     * @param {string} content - HTML content for tooltip
     * @param {string} cssClass - CSS class for tooltip type (e.g., 'state-tooltip-temp', 'dag-tooltip-temp')
     * @param {string} nodeId - Optional node ID to track which node this tooltip is for
     */
    showTooltip(event, content, cssClass = 'tooltip-temp', nodeId = null) {
        // If Ctrl/Cmd is pressed, create a new pinned tooltip
        if (this.tooltipPinned) {
            if (!this.pinnedDuringHover) {
                this.createPinnedTooltip(event, content, cssClass.replace('-temp', '-pinned'));
                this.pinnedDuringHover = true;
            }
            return;
        }

        // Track which node this tooltip is for
        this.currentNodeId = nodeId;

        // Otherwise, show temporary tooltip
        if (!this.currentTooltip) {
            this.currentTooltip = d3.select('body').append('div')
                .attr('class', `tooltip ${cssClass}`)
                .style('position', 'fixed')
                .style('pointer-events', 'none');
        }

        this.currentTooltip
            .html(content)
            .classed('hidden', false);
        
        // Calculate optimal position after content is set (so we can measure it)
        // Use a small delay to ensure DOM has updated
        setTimeout(() => {
            if (this.currentTooltip && !this.tooltipPinned) {
                const tooltipNode = this.currentTooltip.node();
                const position = this.calculateTooltipPosition(event, tooltipNode);
                this.currentTooltip
                    .style('left', `${position.left}px`)
                    .style('top', `${position.top}px`);
            }
        }, 0);
    }

    /**
     * Create a pinned, draggable tooltip
     * @param {MouseEvent} event - Mouse event
     * @param {string} content - HTML content for tooltip
     * @param {string} cssClass - CSS class for pinned tooltip
     */
    createPinnedTooltip(event, content, cssClass = 'tooltip-pinned') {
        const tooltipId = `tooltip-${this.tooltipCounter++}`;

        const tooltip = d3.select('body').append('div')
            .attr('id', tooltipId)
            .attr('class', `tooltip ${cssClass}`)
            .style('position', 'fixed')
            .style('pointer-events', 'auto')
            .style('cursor', 'move');
        
        // Add content first so we can measure it
        tooltip.append('div')
            .attr('class', 'tooltip-content')
            .html(content);
        
        // Calculate optimal position after content is added
        setTimeout(() => {
            const tooltipNode = tooltip.node();
            const position = this.calculateTooltipPosition(event, tooltipNode);
            tooltip
                .style('left', `${position.left}px`)
                .style('top', `${position.top}px`);
        }, 0);

        // Add close button
        tooltip.append('button')
            .attr('class', 'tooltip-close')
            .html('&times;')
            .on('click', () => {
                tooltip.remove();
                this.activeTooltips = this.activeTooltips.filter(t => t.id !== tooltipId);
            });

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
     * Calculate optimal tooltip position to keep it within viewport
     * @param {MouseEvent} event - Mouse event
     * @param {HTMLElement} tooltipElement - Tooltip DOM element
     * @returns {{left: number, top: number}} Optimal position
     */
    calculateTooltipPosition(event, tooltipElement) {
        const offset = 10;
        const padding = 10; // Minimum padding from viewport edges
        const viewportWidth = window.innerWidth;
        const viewportHeight = window.innerHeight;
        
        // Get tooltip dimensions (use a temporary measurement if not yet rendered)
        let tooltipWidth = 300; // Default width
        let tooltipHeight = 200; // Default height
        
        if (tooltipElement) {
            const rect = tooltipElement.getBoundingClientRect();
            if (rect.width > 0 && rect.height > 0) {
                tooltipWidth = rect.width;
                tooltipHeight = rect.height;
            }
        }
        
        // Calculate initial position (below and to the right of cursor)
        // Use clientX/clientY for viewport coordinates since tooltips use position: fixed
        let left = event.clientX + offset;
        let top = event.clientY + offset;
        
        // Check if tooltip would go off the right edge
        if (left + tooltipWidth + padding > viewportWidth) {
            // Position to the left of cursor instead
            left = event.clientX - tooltipWidth - offset;
            // Ensure it doesn't go off the left edge
            if (left < padding) {
                left = padding;
            }
        }
        
        // Check if tooltip would go off the bottom edge
        if (top + tooltipHeight + padding > viewportHeight) {
            // Position above cursor instead
            top = event.clientY - tooltipHeight - offset;
            // Ensure it doesn't go off the top edge
            if (top < padding) {
                top = padding;
            }
        }
        
        // Final bounds check - ensure tooltip stays within viewport
        left = Math.max(padding, Math.min(left, viewportWidth - tooltipWidth - padding));
        top = Math.max(padding, Math.min(top, viewportHeight - tooltipHeight - padding));
        
        return { left, top };
    }

    /**
     * Move tooltip to follow mouse
     * @param {MouseEvent} event - Mouse event
     */
    moveTooltip(event) {
        if (this.currentTooltip && !this.tooltipPinned) {
            const tooltipNode = this.currentTooltip.node();
            const position = this.calculateTooltipPosition(event, tooltipNode);
            
            this.currentTooltip
                .style('left', `${position.left}px`)
                .style('top', `${position.top}px`);
        }
    }

    /**
     * Hide tooltip (unless pinned)
     */
    hideTooltip() {
        if (!this.tooltipPinned && this.currentTooltip) {
            this.currentTooltip.classed('hidden', true);
        }
        // Clear the current node ID when hiding tooltip
        this.currentNodeId = null;
        // Reset the flag when mouse leaves the node
        this.pinnedDuringHover = false;
    }

    /**
     * Update the content of the current tooltip
     * @param {string} content - New HTML content
     */
    updateTooltipContent(content) {
        if (this.currentTooltip) {
            this.currentTooltip.html(content);
        }
    }

    /**
     * Get the ID of the node currently shown in the tooltip
     * @returns {string|null} Current node ID or null
     */
    getCurrentTooltipNode() {
        return this.currentNodeId;
    }

    /**
     * Format timestamp for display
     * @param {number} ts - Timestamp in nanoseconds
     * @returns {string} Formatted timestamp
     */
    static formatTimestamp(ts) {
        if (!ts) return 'N/A';
        const date = new Date(ts / 1000000);
        return date.toLocaleString();
    }

    /**
     * Format hash with expand/collapse functionality
     * @param {string} hash - Hash string
     * @param {string} label - Label for the hash
     * @returns {string} HTML for formatted hash
     */
    static formatHash(hash, label) {
        if (!hash) return 'N/A';
        const shortHash = `${hash.substring(0, 8)}...${hash.substring(hash.length - 8)}`;
        const id = `hash-${label.replace(/\s+/g, '-').toLowerCase()}-${Math.random().toString(36).substring(2, 11)}`;
        return `
            <span class="hash-preview">
                <span class="hash-short" id="${id}">${shortHash}</span>
                <span class="hash-full hidden" id="${id}-full">${hash}</span>
                <button class="hash-toggle" onclick="
                    const short = document.getElementById('${id}');
                    const full = document.getElementById('${id}-full');
                    const isExpanded = full.classList.contains('hidden');
                    if (isExpanded) {
                        short.classList.add('hidden');
                        full.classList.remove('hidden');
                        this.textContent = '▼';
                    } else {
                        short.classList.remove('hidden');
                        full.classList.add('hidden');
                        this.textContent = '▶';
                    }
                ">▶</button>
            </span>
        `;
    }
}
