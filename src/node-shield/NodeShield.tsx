import React, { useContext, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { CalimeroClient } from '@calimero-network/mero-js';
import { MeroContext } from '../context/context';
import { useAppCode, useNodeVerification, type AppCodeChecker } from './nodeChecks';
import { describeShield, shieldDetails, type ShieldKind, type ShieldView } from './shieldView';
import './nodeShield.css';

const EDGE_PX = 12; // the popover's distance from the viewport's edge
const GAP_PX = 8; // and from the chip
const COPIED_MS = 2_000; // how long "Copied" shows
// Layout effects warn on the server, where there is no layout to measure.
const useClientLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;
const COLOR_KEYS = ['verified', 'refused', 'checking', 'neutral', 'surface', 'text', 'muted', 'border', 'focus'] as const;

/** Colours for the shield; each sets the `--node-shield-<key>` CSS custom property of the same name. */
export type NodeShieldColors = Partial<Record<(typeof COLOR_KEYS)[number], string>>;

export interface NodeShieldProps {
  /** The client whose node is checked; by default the `MeroProvider`'s. `null` shows nothing. */
  client?: CalimeroClient | null;
  /** The label shown as "Cluster"; by default the `MeroProvider`'s cluster. `null` leaves the row out. */
  cluster?: string | null;
  /** Icon only when the window is this many px wide or narrower (always at 560px or narrower). */
  compactBelow?: number;
  /** The app's own check that node `url` runs its code, asked when the popover opens on a real, verified node. Memoise it. */
  appCode?: AppCodeChecker;
  /** Colours, as an alternative to setting the `--node-shield-*` custom properties in CSS. */
  colors?: NodeShieldColors;
}

const cssName = (key: string) => `--node-shield-${key}`;

function colorVars(colors: NodeShieldColors | undefined): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const key of COLOR_KEYS) if (colors?.[key]) vars[cssName(key)] = colors[key]!;
  return vars;
}

/** The colour properties the chip resolves from its ancestors, for the popover, which renders in `document.body`. */
function inheritedVars(el: HTMLElement | null): Record<string, string> {
  const vars: Record<string, string> = {};
  if (!el) return vars;
  const style = getComputedStyle(el);
  for (const key of COLOR_KEYS) {
    const value = style.getPropertyValue(cssName(key)).trim();
    if (value) vars[cssName(key)] = value;
  }
  return vars;
}

type CopyState = 'idle' | 'done' | 'failed';

/** Copies text to the clipboard; the result shows for two seconds. Never throws, with or without a clipboard. */
function useCopy(): [CopyState, (text: string) => void] {
  const [state, setState] = useState<CopyState>('idle');
  useEffect(() => {
    if (state === 'idle') return;
    const timer = setTimeout(() => setState('idle'), COPIED_MS);
    return () => clearTimeout(timer);
  }, [state]);
  const copy = (text: string) =>
    void Promise.resolve()
      .then(() => navigator.clipboard.writeText(text))
      .then(
        () => setState('done'),
        () => setState('failed'),
      );
  return [state, copy];
}

type Placement = { top: number; left: number; maxHeight: number } | null;

/** Right edges aligned with the chip, below it or above when that is where the room is, inside the viewport. */
function usePlacement(open: boolean, chip: React.RefObject<HTMLElement>, pop: React.RefObject<HTMLElement>): Placement {
  const [at, setAt] = useState<Placement>(null);
  useClientLayoutEffect(() => {
    if (!open) return setAt(null);
    const place = () => {
      if (!chip.current || !pop.current) return;
      const anchor = chip.current.getBoundingClientRect();
      const { offsetWidth: width, scrollHeight: height } = pop.current;
      const below = window.innerHeight - anchor.bottom - GAP_PX - EDGE_PX;
      const above = anchor.top - GAP_PX - EDGE_PX;
      const room = height <= below || below >= above ? below : above;
      setAt({
        top: room === below ? anchor.bottom + GAP_PX : anchor.top - GAP_PX - Math.min(height, room),
        left: Math.min(Math.max(anchor.right - width, EDGE_PX), window.innerWidth - width - EDGE_PX),
        maxHeight: Math.max(room, 0),
      });
    };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    // The popover grows when the app code check arrives: re-place it so it stays clear of the chip.
    const grown = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(place);
    if (pop.current) grown?.observe(pop.current);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
      grown?.disconnect();
    };
  }, [open, chip, pop]);
  return at;
}

/** Whether the window is `width` px wide or narrower; never without a width or a `matchMedia`. */
function useNarrowerThan(width: number | undefined): boolean {
  const [narrow, setNarrow] = useState(false);
  // Measured after mount, never during render: the server and the first browser render agree.
  useClientLayoutEffect(() => {
    const query = width === undefined || typeof window.matchMedia !== 'function' ? null : `(max-width: ${width}px)`;
    if (!query) return setNarrow(false);
    const media = window.matchMedia(query);
    const update = () => setNarrow(media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, [width]);
  return narrow;
}

function ShieldIcon({ kind }: { kind: ShieldKind }) {
  const outline = 'M12 2 4 5v6c0 5 3.4 9.4 8 11 4.6-1.6 8-6 8-11V5l-8-3z';
  return (
    <svg className="mero-node-shield__icon" viewBox="0 0 24 24" width="16" height="16" fill="none" aria-hidden="true">
      <path d={outline} fill="currentColor" opacity=".18" />
      <path d={outline} stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
      {kind === 'verified' && <path d="m8.5 12 2.4 2.4L15.5 9.8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />}
      {kind === 'refused' && <path d="m9.5 9.5 5 5m0-5-5 5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />}
      {kind === 'unsealed' && <path d="M12 8v4.5M12 15.6v.1" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />}
    </svg>
  );
}

const Tick = () => (
  <svg className="mero-node-shield__tick" viewBox="0 0 24 24" width="14" height="14" fill="none" aria-hidden="true">
    <path d="m5 12.5 4.5 4.5L19 7.5" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

const ExternalIcon = () => (
  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" aria-hidden="true">
    <path d="M14 4h6v6m0-6-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

interface PopoverProps {
  view: ShieldView;
  id: string;
  style: React.CSSProperties;
  popRef: React.RefObject<HTMLDivElement>;
}

function Popover({ view, id, style, popRef }: PopoverProps) {
  const [copied, copy] = useCopy();
  return createPortal(
    <div
      ref={popRef}
      id={id}
      role="dialog"
      aria-labelledby={`${id}-title`}
      tabIndex={-1}
      className="mero-node-shield-pop"
      data-kind={view.kind}
      style={style}
      data-testid="node-shield-popover"
    >
      <h3 id={`${id}-title`} className="mero-node-shield-pop__title">
        <ShieldIcon kind={view.kind} />
        {view.title}
      </h3>
      <p className="mero-node-shield-pop__sub">{view.sub}</p>
      <dl className="mero-node-shield-pop__rows">
        {view.rows.map((row) => (
          <div key={row.label} className={`mero-node-shield-pop__row${row.tone === 'bad' ? ' mero-node-shield-pop__row--bad' : ''}`}>
            <dt>{row.label}</dt>
            <dd className={row.mono ? 'mero-node-shield-pop__mono' : undefined} title={row.full}>
              {row.hash ? (
                <>
                  {row.value.slice(0, -row.hash.length)}
                  <span className="mero-node-shield-pop__nowrap">{row.hash}</span>
                </>
              ) : (
                row.value
              )}
            </dd>
          </div>
        ))}
      </dl>
      {view.checks.length > 0 && (
        <ul className="mero-node-shield-pop__checks">
          {view.checks.map((text) => (
            <li key={text}>
              <Tick />
              {text}
            </li>
          ))}
        </ul>
      )}
      <div className="mero-node-shield-pop__buttons">
        {view.verifyUrl && (
          <a className="mero-node-shield-pop__btn mero-node-shield-pop__btn--primary" href={view.verifyUrl} target="_blank" rel="noopener noreferrer">
            Verify independently
            <ExternalIcon />
          </a>
        )}
        <button type="button" className="mero-node-shield-pop__btn" onClick={() => copy(shieldDetails(view))}>
          {copied === 'done' ? 'Copied' : copied === 'failed' ? 'Could not copy' : 'Copy details'}
        </button>
      </div>
      {view.verifyUrl && <p className="mero-node-shield-pop__foot">Opens the public verifier, which re-checks this node from outside the app.</p>}
    </div>,
    document.body,
  );
}

/**
 * A header chip for the node `client` talks to, with a popover of what the browser's check verified.
 * Renders nothing until there is a client.
 */
export function NodeShield({ client: clientProp, cluster: clusterProp, compactBelow, appCode, colors }: NodeShieldProps) {
  const context = useContext(MeroContext);
  const client = clientProp !== undefined ? clientProp : (context?.client ?? null);
  const cluster = clusterProp !== undefined ? clusterProp : (context?.cluster ?? null);
  const [state, refresh] = useNodeVerification(client);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  const chip = useRef<HTMLButtonElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  const popId = useId();
  const placement = usePlacement(open, chip, pop);
  const compact = useNarrowerThan(compactBelow);
  const real = state.status === 'verified' && !state.mock;
  const code = useAppCode(client, appCode, open && real ? state.url : null);

  useEffect(() => {
    if (!open) return;
    refresh();
    const close = (refocus: boolean) => {
      setOpen(false);
      if (refocus) chip.current?.focus();
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close(true);
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!chip.current?.contains(target) && !pop.current?.contains(target)) close(false);
    };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onDown);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
    };
  }, [open, refresh]);

  // Focus waits for the placement: a popover still hidden for it cannot take focus.
  const placed = placement !== null;
  useEffect(() => {
    if (open && placed) pop.current?.focus();
  }, [open, placed]);

  if (!client) return null;
  const view = describeShield(state, { cluster, code });
  const vars = colorVars(colors);
  return (
    <span ref={root} className={`mero-node-shield${compact ? ' mero-node-shield--compact' : ''}`} style={vars}>
      <button
        ref={chip}
        type="button"
        className="mero-node-shield__chip"
        data-kind={view.kind}
        aria-label={view.label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popId : undefined}
        onClick={() => setOpen((v) => !v)}
        data-testid="node-shield"
      >
        <ShieldIcon kind={view.kind} />
        <span className="mero-node-shield__label">{view.label}</span>
      </button>
      {open && (
        <Popover
          view={view}
          id={popId}
          popRef={pop}
          style={{ ...inheritedVars(root.current), ...vars, ...(placement ?? { visibility: 'hidden' }) }}
        />
      )}
    </span>
  );
}
