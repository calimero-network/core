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
    }

    /**
     * Clean up all active tooltips
     */
    cleanupTooltips() {
        this.activeTooltips.forEach(tooltip => {
            if (tooltip.element && tooltip.element.parentNode) {
                tooltip.element.parentNode.removeChild(tooltip.element);
            }
        });
        this.activeTooltips = [];
        this.currentTooltip = null;
    }

    /**
     * Show tooltip with content
     * @param {MouseEvent} event - Mouse event
     * @param {string} content - HTML content for tooltip
     * @param {string} cssClass - CSS class for tooltip type (e.g., 'state-tooltip-temp', 'dag-tooltip-temp')
     */
    showTooltip(event, content, cssClass = 'tooltip-temp') {
        // If Ctrl/Cmd is pressed, create a new pinned tooltip
        if (this.tooltipPinned) {
            if (!this.pinnedDuringHover) {
                this.createPinnedTooltip(event, content, cssClass.replace('-temp', '-pinned'));
                this.pinnedDuringHover = true;
            }
            return;
        }

        // Otherwise, show temporary tooltip
        if (!this.currentTooltip) {
            this.currentTooltip = d3.select('body').append('div')
                .attr('class', `tooltip ${cssClass}`)
                .style('position', 'fixed')
                .style('pointer-events', 'none');
        }

        this.currentTooltip
            .html(content)
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`)
            .classed('hidden', false);
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
            .style('left', `${event.pageX + 10}px`)
            .style('top', `${event.pageY + 10}px`)
            .style('pointer-events', 'auto')
            .style('cursor', 'move');

        // Add close button
        tooltip.append('button')
            .attr('class', 'tooltip-close')
            .html('&times;')
            .on('click', () => {
                tooltip.remove();
                this.activeTooltips = this.activeTooltips.filter(t => t.id !== tooltipId);
            });

        // Add content
        tooltip.append('div')
            .attr('class', 'tooltip-content')
            .html(content);

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
