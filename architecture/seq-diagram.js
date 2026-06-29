/* Calimero Core — shared animated sequence-diagram engine.
 *
 * Extracted from sequence-diagrams.html so any page can render the same
 * step-cascade animated SVGs from data instead of hand-placed coordinates.
 *
 * Usage on a page:
 *   <div class="diagram-tools"><button class="replay-btn"
 *        onclick="replay('d-foo')">&#9654; Replay</button></div>
 *   <div class="diagram-container animating" id="d-foo"></div>
 *   <script src="../seq-diagram.js"></script>
 *   <script>
 *     window.SEQ_DIAGRAMS = [{ id:'d-foo', title:'Foo', lanes:[...], steps:[...] }];
 *   </script>
 *
 * A `lane`  = { label, color }.
 * A `step`  = { from, to, label, dashed?, color? }  (from===to draws a self-call)
 *           | { type:'divider', label }.
 *
 * The renderer wraps each step in <g class="seq-step" style="--step-i:N">; the
 * cascade animation lives in styles.css and fires whenever the container has
 * the `animating` class. Define SEQ_DIAGRAMS either before or after this script.
 */
(function () {
  'use strict';

  var NS = 'http://www.w3.org/2000/svg';

  function S(tag, a, txt) {
    var e = document.createElementNS(NS, tag);
    if (a) Object.keys(a).forEach(function (k) { e.setAttribute(k, a[k]); });
    if (txt != null) e.textContent = txt;
    return e;
  }

  function render(id, data) {
    var box = document.getElementById(id);
    if (!box) return;
    box.textContent = '';

    var lanes = data.lanes, steps = data.steps;
    var n = lanes.length, m = steps.length;

    var L = 48, R = 18;
    var W = Math.max(960, L + n * 150 + R);
    var SS = 90, SH = 52;
    var H = SS + m * SH + 28;
    var lw = (W - L - R) / n;
    var cx = lanes.map(function (_, i) { return L + lw * i + lw / 2; });

    var svg = S('svg', { viewBox: '0 0 ' + W + ' ' + H, role: 'img', 'aria-label': data.title + ' sequence diagram' });
    svg.appendChild(S('title', null, data.title));
    svg.appendChild(S('rect', { x: 0, y: 0, width: W, height: H, fill: '#0f0f16', rx: 10 }));

    lanes.forEach(function (lane, i) {
      var x = cx[i], hw = Math.min(lw - 10, 134);
      svg.appendChild(S('rect', { x: x - hw / 2, y: 10, width: hw, height: 32, rx: 6, fill: lane.color + '18', stroke: lane.color + '50', 'stroke-width': 1 }));
      svg.appendChild(S('text', { x: x, y: 31, 'text-anchor': 'middle', fill: lane.color, 'font-family': "'JetBrains Mono', monospace", 'font-size': lane.label.length > 11 ? '9.5' : '10.5', 'font-weight': '600' }, lane.label));
      svg.appendChild(S('line', { x1: x, y1: 48, x2: x, y2: H - 14, stroke: '#252538', 'stroke-width': 1, 'stroke-dasharray': '3,4' }));
    });

    var stepNum = 0;
    steps.forEach(function (step, i) {
      var y = SS + i * SH;
      var g = S('g', { class: 'seq-step', style: '--step-i:' + i });

      if (step.type === 'divider') {
        g.appendChild(S('line', { x1: L, y1: y, x2: W - R, y2: y, stroke: '#35354a', 'stroke-width': 1, 'stroke-dasharray': '6,4' }));
        var dlw = step.label.length * 6.2 + 24;
        g.appendChild(S('rect', { x: W / 2 - dlw / 2, y: y - 11, width: dlw, height: 20, fill: '#0f0f16', rx: 4 }));
        g.appendChild(S('text', { x: W / 2, y: y + 4, 'text-anchor': 'middle', fill: '#6b6b80', 'font-family': "'DM Sans', sans-serif", 'font-size': '10.5', 'font-style': 'italic' }, step.label));
        svg.appendChild(g);
        return;
      }

      stepNum++;
      g.appendChild(S('text', { x: 12, y: y + 4, 'text-anchor': 'start', fill: '#4a4a5a', 'font-family': "'JetBrains Mono', monospace", 'font-size': '9.5', 'font-weight': '700' }, String(stepNum)));

      var fx = cx[step.from], tx = cx[step.to];
      var col = step.color || lanes[step.to].color;

      if (step.from === step.to) {
        var rightSide = step.from < n - 1;
        var dir = rightSide ? 1 : -1;
        var lp = 36, lh = 18;
        var sx = fx + dir * 3;

        g.appendChild(S('path', {
          d: 'M ' + sx + ',' + (y - lh / 2) + ' h ' + (dir * lp) + ' v ' + lh + ' h ' + (dir * -lp),
          fill: 'none', stroke: col, 'stroke-width': 1.3
        }));

        var ax = sx, ay = y + lh / 2;
        g.appendChild(S('polygon', { points: (ax + dir * 5) + ',' + (ay - 3.5) + ' ' + ax + ',' + ay + ' ' + (ax + dir * 5) + ',' + (ay + 3.5), fill: col }));

        var lx = fx + dir * (lp + 10);
        g.appendChild(S('text', { x: lx, y: y + 3, 'text-anchor': rightSide ? 'start' : 'end', fill: col, 'font-family': "'JetBrains Mono', monospace", 'font-size': '9', 'font-weight': '500' }, step.label));
      } else {
        var d = tx > fx ? 1 : -1;
        var x1 = fx + d * 5, x2 = tx - d * 5;

        g.appendChild(S('circle', { cx: fx, cy: y, r: 2.5, fill: col + '70' }));
        g.appendChild(S('circle', { cx: tx, cy: y, r: 3, fill: col }));

        var dashVal = step.dashed ? '5,3' : 'none';
        var opVal = step.dashed ? '.55' : '1';
        g.appendChild(S('line', { x1: x1, y1: y, x2: x2, y2: y, stroke: col, 'stroke-width': 1.5, 'stroke-dasharray': dashVal, 'stroke-opacity': opVal }));
        g.appendChild(S('polygon', { points: x2 + ',' + y + ' ' + (x2 - d * 7) + ',' + (y - 3.5) + ' ' + (x2 - d * 7) + ',' + (y + 3.5), fill: col, 'fill-opacity': opVal }));

        var mx = (x1 + x2) / 2;
        g.appendChild(S('text', { x: mx, y: y - 8, 'text-anchor': 'middle', fill: '#c4c4cc', 'font-family': "'JetBrains Mono', monospace", 'font-size': '9.5' }, step.label));
      }

      svg.appendChild(g);
    });

    box.appendChild(svg);
  }

  function replay(id) {
    var el = document.getElementById(id);
    if (!el) return;
    el.classList.remove('animating');
    void el.offsetWidth;          // force reflow so the animation re-runs
    el.classList.add('animating');
  }

  window.replay = replay;
  window.renderSeq = render;

  document.addEventListener('DOMContentLoaded', function () {
    (window.SEQ_DIAGRAMS || []).forEach(function (d) { render(d.id, d); });
  });
})();
