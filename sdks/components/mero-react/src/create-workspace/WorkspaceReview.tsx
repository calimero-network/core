import "./workspaceReview.css";
import { useEffect, useId, useRef, useState } from "react";
import type { CalimeroClient, PackageCapabilities } from "@calimero-network/mero-js";

import { useCard, useNodeVerification, type ReviewRelease, type UnreleasedApp } from "../node-shield/nodeChecks";
import { ExternalIcon, Icon, ShieldIcon, Tick, type IconName } from "./shieldIcons";
import { describeShield, hostOf, type ShieldKind } from "../node-shield/shieldView";
import { HostText } from "./HostText";
import { useCopy } from "./useCopy";
import {
  abiFileName,
  actionCount,
  actionGroups,
  codeLine,
  dataStatements,
  exceptions,
  rawFacts,
  shortSigner,
  summarize,
  type ActionGroup,
  type ActionView,
  type GroupId,
  type SummaryLine,
} from "./workspaceFacts";

const OPEN_GROUP_MAX = 3; // a longer group starts closed

const SUMMARY_ICON = (line: SummaryLine): IconName => {
  const icons: Record<SummaryLine["id"], IconName> = { passkey: "key", public: "globe", publicCalls: "globe", outside: "out", timers: "clock", apps: line.tone === "warn" ? "link" : "check" };
  return icons[line.id];
};
const GROUP_ICON: Record<GroupId, IconName> = { passkey: "key", public: "globe", changes: "pencil", reads: "eye", timers: "clock", outside: "out", events: "bell", other: "dots" };

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, copy] = useCopy();
  return (
    <button type="button" className="mero-cw-review__copy" aria-label={label} onClick={() => copy(text)}>
      {copied === "done" ? "Copied" : copied === "failed" ? "Could not copy" : "Copy"}
    </button>
  );
}

function downloadAbi(card: PackageCapabilities) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(card.abi, null, 2)], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = abiFileName(card);
  link.click();
  setTimeout(() => URL.revokeObjectURL(url));
}

function Facts({ facts }: { facts: Array<{ label: string; value: string; mono?: boolean }> }) {
  return (
    <dl className="mero-cw-review__kv">
      {facts.map((f) => (
        <div key={f.label}>
          <dt>{f.label}</dt>
          <dd className={f.mono ? "mero-cw-review__mono" : undefined}>{f.value}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A doc as plain text: `code` spans become <code>, everything else stays text. */
function DocText({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : part))}
    </>
  );
}

function ActionRow({ item, mono }: { item: ActionView; mono: boolean }) {
  const title = (
    <span className="mero-cw-review__act-main">
      <span className={mono ? "mero-cw-review__act-title mero-cw-review__mono" : "mero-cw-review__act-title"}>{item.title}</span>
      {item.summary && (
        <span className="mero-cw-review__act-doc" data-publisher="">
          <DocText text={item.summary} />
        </span>
      )}
    </span>
  );
  const tags = (
    <span className="mero-cw-review__tags">
      {item.tags.map((tag) => (
        <span key={tag} className={`mero-cw-review__tag mero-cw-review__tag--${tag.split(" ")[0]}`}>
          {tag}
        </span>
      ))}
    </span>
  );
  if (item.facts.length === 0 && !item.doc) {
    return (
      <div className="mero-cw-review__act mero-cw-review__act--flat">
        <div className="mero-cw-review__act-head">
          {title}
          {tags}
        </div>
      </div>
    );
  }
  return (
    <details className="mero-cw-review__act">
      <summary>
        {title}
        {tags}
      </summary>
      <div className="mero-cw-review__facts">
        <Facts facts={item.facts} />
        {item.doc && (
          <div className="mero-cw-review__said" data-publisher="">
            <span>Publisher&apos;s description</span>
            {item.doc.split("\n\n").map((paragraph) => (
              <p key={paragraph}>
                <DocText text={paragraph} />
              </p>
            ))}
          </div>
        )}
      </div>
    </details>
  );
}

function GroupView({ group, filtered }: { group: ActionGroup; filtered: boolean }) {
  return (
    <details className="mero-cw-review__grp" open={filtered || group.items.length <= OPEN_GROUP_MAX}>
      <summary>
        <Icon name={GROUP_ICON[group.id]} />
        <span className="mero-cw-review__grp-text">
          <b>{group.label}</b>
          <span>{group.blurb}</span>
        </span>
        <span className="mero-cw-review__count">{group.items.length}</span>
      </summary>
      <div className="mero-cw-review__acts">
        {group.items.map((item) => (
          <ActionRow key={item.name} item={item} mono={group.id === "outside"} />
        ))}
      </div>
    </details>
  );
}

function AllActions({ card, signed }: { card: PackageCapabilities; signed: boolean }) {
  const groups = actionGroups(card);
  const [only, setOnly] = useState<GroupId | null>(null);
  const count = actionCount(card.abi);
  if (groups.length === 0) return null;
  return (
    <details className="mero-cw-review__more">
      <summary>
        <span>{count > 0 ? `See all ${count} ${count === 1 ? "action" : "actions"}` : "See details"}</span>
      </summary>
      <div className="mero-cw-review__chips" role="group" aria-label="Filter actions">
        <button type="button" className="mero-cw-review__chip" aria-pressed={only === null} onClick={() => setOnly(null)}>
          All
        </button>
        {groups.map((g) => (
          <button key={g.id} type="button" className="mero-cw-review__chip" aria-pressed={only === g.id} onClick={() => setOnly(g.id)}>
            {`${g.chip} (${g.items.length})`}
          </button>
        ))}
      </div>
      {groups
        .filter((g) => only === null || g.id === only)
        .map((g) => (
          <GroupView key={g.id} group={g} filtered={only !== null} />
        ))}
      <p className="mero-cw-review__note">Tags and facts come from {signed ? "the app's signed release" : "the release as this test server lists it"}. The publisher&apos;s description is the publisher&apos;s own words and is not checked.</p>
    </details>
  );
}

function SignedFacts({ card, signed }: { card: PackageCapabilities; signed: boolean }) {
  return (
    <details className="mero-cw-review__more">
      <summary>
        <span>{signed ? "Signed facts" : "Facts as this test server lists them"}</span>
      </summary>
      <dl className="mero-cw-review__kv mero-cw-review__kv--raw">
        {rawFacts(card).map((f) => (
          <div key={f.label}>
            <dt>{f.label}</dt>
            <dd className="mero-cw-review__mono">{f.value}</dd>
          </div>
        ))}
      </dl>
      {card.abi ? (
        <button type="button" className="mero-cw-review__btn" onClick={() => downloadAbi(card)}>
          Download full ABI (JSON)
        </button>
      ) : (
        <p className="mero-cw-review__muted">There is no action list to download.</p>
      )}
    </details>
  );
}

function Section({ title, kind, children }: { title: string; kind?: React.ComponentProps<typeof ShieldIcon>["kind"]; children: React.ReactNode }) {
  const id = useId();
  return (
    <section className="mero-cw-review__sec" aria-labelledby={id}>
      <h3 id={id} className="mero-cw-review__h" data-kind={kind}>
        {kind && <ShieldIcon kind={kind} />}
        {title}
      </h3>
      {children}
    </section>
  );
}

/** What a review is built from: the node and its check, and the release's card. */
function useReview(client: CalimeroClient | null, release: ReviewRelease | UnreleasedApp) {
  const [state] = useNodeVerification(client);
  const [loaded, retry] = useCard(client, release, state);
  return { client, state, loaded, retry };
}

type Review = ReturnType<typeof useReview>;

function Diverged({ divergence }: { divergence: "release" | "code" }) {
  return (
    <p className="mero-cw-review__grey" data-kind="refused">
      {divergence === "release" ? (
        <>
          <b>The node runs another release than this one</b>
          <span>No statement about your data is made here.</span>
        </>
      ) : (
        <>
          <b>Code does not match the release</b>
          <span>This node runs different code than the release, so no statement about your data is made here.</span>
        </>
      )}
    </p>
  );
}

const STATUS: Record<ShieldKind, string> = { verified: "Verified", checking: "Checking…", unsealed: "Not sealed", refused: "Refused" };
/** Summary lines that the exceptions under "Your data" already name. */
const IN_EXCEPTIONS = new Set<SummaryLine["id"]>(["public", "publicCalls", "outside"]);

/** The visible review: the app, where it runs, what happens to the data and what the app can do. Everything else waits under Technical details. */
function ReviewBody({ review, release, cluster, noun }: { review: Review; release: ReviewRelease | UnreleasedApp; cluster?: string; noun: string }) {
  const { client, state, loaded, retry } = review;
  const view = describeShield(state, { code: null });
  const card = loaded.status === "ready" ? loaded.card : null;
  const sealed = view.kind === "verified";
  const nodeRelease = state.status === "verified" && !state.mock ? state.release : undefined;
  const code = card ? codeLine(card, null, sealed) : null;
  const risks = card ? exceptions(card, sealed) : [];
  const summary = card ? summarize(card, sealed).filter((line) => !(risks.length > 0 && IN_EXCEPTIONS.has(line.id))) : [];
  const statements = dataStatements(view.kind, "creator", noun);
  const unreleased = loaded.status === 'none';
  const unknown = unreleased
    ? "This build has no signed release, so what it can do and what leaves the sealed hardware is not known."
    : !card
    ? "What leaves the sealed hardware is not known until the app's details load."
    : !card.abi
      ? "This app publishes no action list, so what it makes visible to anyone is not known."
      : null;
  const appName = card?.name ?? release.package;

  return (
    <>
      {loaded.status === "error" && (
        <div className="mero-cw-review__alert" role="alert">
          <p>
            {loaded.message}
            {client ? ` You can still create the ${noun}.` : ""}
          </p>
          <button type="button" className="mero-cw-review__btn" onClick={retry}>
            Try again
          </button>
        </div>
      )}

      <div className="mero-cw-review__app">
        <span className="mero-cw-review__tile" aria-hidden="true">
          {appName.replace(/^.*\./, "").charAt(0).toUpperCase()}
        </span>
        <span className="mero-cw-review__app-text">
          <b>{appName}</b>
          <span>{unreleased ? "Unreleased build" : `Version ${release.version}`}</span>
        </span>
      </div>

      <Section title="Where it runs">
        <p className="mero-cw-review__where" data-kind={view.kind}>
          <ShieldIcon kind={view.kind} />
          <span className="mero-cw-review__where-name">{cluster ? <HostText text={cluster} /> : "Your node"}</span>
          <span className="mero-cw-review__pill">{STATUS[view.kind]}</span>
        </p>
      </Section>

      <Section title="Your data">
        {code?.divergence ? (
          <Diverged divergence={code.divergence} />
        ) : (
          <>
            {statements.length > 0 && (
              <ul className={sealed ? "mero-cw-review__claims" : "mero-cw-review__claims mero-cw-review__claims--plain"}>
                {statements.map((text) => (
                  <li key={text}>
                    {sealed && <Tick />}
                    <span>{text}</span>
                  </li>
                ))}
              </ul>
            )}
            {!sealed && (
              <p className="mero-cw-review__grey" data-kind={view.kind}>
                <b>{view.title}</b>
                <span>{view.sub}</span>
              </p>
            )}
            {risks.length > 0 && (
              <>
                {!sealed && <p className="mero-cw-review__muted">As this test server lists the app:</p>}
                <ul className="mero-cw-review__except">
                  {risks.map((text) => (
                    <li key={text}>
                      <Icon name="warn" />
                      {text}
                    </li>
                  ))}
                </ul>
              </>
            )}
            {sealed && unknown && <p className="mero-cw-review__muted">{unknown}</p>}
          </>
        )}
      </Section>

      <Section title="What it can do">
        {loaded.status === "loading" && (
          <p className="mero-cw-review__muted" role="status">
            Loading what this app can do…
          </p>
        )}
        {loaded.status === "error" && <p className="mero-cw-review__muted">Not available until the review loads.</p>}
        {unreleased && <p className="mero-cw-review__muted">This app has no published release yet, so there is no signed review to show.</p>}
        {loaded.status === "refused" && <p className="mero-cw-review__muted">Not available: this node did not pass its check.</p>}
        {card && summary.length > 0 && (
          <ul className="mero-cw-review__sum">
            {summary.map((line) => (
              <li key={line.id} data-tone={line.tone}>
                <span className="mero-cw-review__ic">
                  <Icon name={SUMMARY_ICON(line)} />
                </span>
                <span className="mero-cw-review__tx">
                  <b>{line.title}</b>
                  <span>{line.detail}</span>
                </span>
              </li>
            ))}
          </ul>
        )}
        {card && !card.abi && <p className="mero-cw-review__muted">This app publishes no action list.</p>}
      </Section>

      <section className="mero-cw-review__tech" aria-label="Technical details">
        <details className="mero-cw-review__more">
          <summary>
            <span>Technical details</span>
          </summary>
          <dl className="mero-cw-review__kv">
            {card && (
              <div>
                <dt>Publisher</dt>
                <dd>
                  <span className="mero-cw-review__mono" title={card.signer}>
                    {shortSigner(card.signer)}
                  </span>
                  <CopyButton text={card.signer} label="Copy publisher" />
                </dd>
              </div>
            )}
            {code && (
              <div>
                <dt>Code</dt>
                <dd className={code.tone ? `mero-cw-review__tone--${code.tone}` : undefined}>
                  <span>{code.text}</span>
                  {code.note && <span className="mero-cw-review__sub">{code.note}</span>}
                </dd>
              </div>
            )}
            {nodeRelease && (
              <div>
                <dt>Software</dt>
                <dd>
                  <span className="mero-cw-review__mono">{`mero-tee v${nodeRelease}`}</span>
                  {view.verifyUrl && (
                    <a className="mero-cw-review__link" href={view.verifyUrl} target="_blank" rel="noopener noreferrer" title="Opens the public verifier, which re-checks this node from outside the app">
                      Verify
                      <ExternalIcon />
                    </a>
                  )}
                </dd>
              </div>
            )}
            {state.status === "verified" && (
              <div>
                <dt>Node</dt>
                <dd className="mero-cw-review__mono"><HostText text={hostOf(state.url)} /></dd>
              </div>
            )}
          </dl>
          {sealed && !code?.divergence && (
            <div className="mero-cw-review__how">
              <b>How we know</b>
              <ul>
                {nodeRelease && <li>{`Runs Calimero's published, signed node software (release v${nodeRelease})`}</li>}
                {view.checks.map((text) => (
                  <li key={text}>{text}</li>
                ))}
              </ul>
            </div>
          )}
          {card && (
            <>
              <AllActions card={card} signed={sealed} />
              <SignedFacts card={card} signed={sealed} />
            </>
          )}
        </details>
      </section>
    </>
  );
}

export interface WorkspaceReviewProps {
  /** The client of the node the workspace will be created on; `null` while there is none. */
  client: CalimeroClient | null;
  /** The cluster's name, as shown under "Where it runs". */
  cluster?: string;
  /** A local development node: its label already says how many nodes there are. */
  local?: boolean;
  /** What is being created, in the sentences around the review. Default `workspace`. */
  noun?: string;
  /** The package release the workspace will run. */
  release: ReviewRelease | UnreleasedApp;
  /** The workspace is being created: both buttons wait. */
  creating?: boolean;
  /** Create waits for something else (the app is still finding out how to ask the node). */
  disabled?: boolean;
  onBack: () => void;
  onCreate: () => void;
}

/**
 * Step 2 of creating a workspace: what the app is, what it can do, where it
 * runs and what that means for the data, from the release's signed capability
 * card and the node's own check. Read by the person before "Create workspace",
 * which stays disabled while the node has refused its check.
 */
export function WorkspaceReview({ client, cluster, noun = "workspace", release, creating = false, disabled = false, onBack, onCreate }: WorkspaceReviewProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), []);
  const review = useReview(client, release);
  const refused = review.state.status === "refused";
  return (
    <div className="mero-cw-review">
      <h2 ref={heading} tabIndex={-1} className="mero-cw-review__title">
        Review before creating
      </h2>
      <p className="mero-cw-review__lede">{`What this app can do with your ${noun}, and where it runs.`}</p>
      <ReviewBody review={review} release={release} cluster={cluster} noun={noun} />
      {refused && <p className="mero-cw-review__muted" role="note">Create is unavailable: this node did not pass its check.</p>}
      <div className="mero-cw-review__buttons">
        <button type="button" className="mero-cw-review__btn" disabled={creating} onClick={onBack}>
          Back
        </button>
        <button type="button" className="mero-cw-review__btn mero-cw-review__btn--primary" disabled={creating || disabled || refused} onClick={onCreate}>
          {creating ? "Creating…" : `Create ${noun}`}
        </button>
      </div>
    </div>
  );
}
