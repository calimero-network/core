/** What a workspace review says about an app, derived from its signed capability card: only what the card backs. */
import type { AppAbi, AppAbiMethod, PackageCapabilities } from "@calimero-network/mero-js";
import type { AppCodeCheck } from "../node-shield/shieldView";

import { hostOf, shortHash, type ShieldKind } from "../node-shield/shieldView";

const LIST_SHOWN = 3;
const SIGNER_SHORT_AFTER = 16;
const SIGNER_HEAD = 12;
const SIGNER_TAIL = 4;
const EVERY_UNITS: Record<string, string> = { s: "second", m: "minute", h: "hour" };
const INVITED_ONLY = "Only people invited to it can open it";
const NOT_READABLE = "Calimero can't read it.";
const TEST_SERVER_ENFORCES = ", as this test server enforces it";
const control = (viewer: "creator" | "member", noun: string) => (viewer === "creator" ? "Only you control it" : `Only the ${noun}'s owners control it`);
export const MAX_DOC_CHARS = 4000; // a publisher's description is cut here before it is parsed or shown
const WORD_TAIL = 8; // how far back a sentence end looks for an abbreviation
const ABBREVIATIONS = new Set(["e.g", "i.e", "etc", "vs", "cf"]); // a full stop after these does not end a sentence
const CALLED_KINDS = new Set(["call", "view", "action"]); // what a caller can start directly

/** What the review reads from a capability card. */
export type ReviewCard = Pick<PackageCapabilities, "outbound" | "abi">;

export interface SummaryLine {
  id: "passkey" | "public" | "publicCalls" | "outside" | "timers" | "apps";
  title: string;
  detail: string;
  tone?: "warn" | "ok";
}

export interface ActionFact {
  label: string;
  value: string;
  mono?: boolean;
}

export interface ActionView {
  name: string;
  title: string;
  /** The publisher's own words: not a fact the node enforces. */
  doc?: string;
  /** The first sentence of `doc`, for the row. */
  summary?: string;
  tags: string[];
  facts: ActionFact[];
}

export type GroupId = "passkey" | "public" | "changes" | "reads" | "timers" | "outside" | "events" | "other";

export interface ActionGroup {
  id: GroupId;
  label: string;
  chip: string;
  blurb: string;
  items: ActionView[];
}

/** `deleteWorkspace` and `delete_workspace` read "Delete workspace". */
export function humanize(name: string): string {
  const words = name
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .trim()
    .toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** `1h` reads "1 hour"; an interval that is not `<n>s|m|h` is shown as written. */
function everyWords(every: string): string {
  const match = /^(\d+)([smh])$/.exec(every);
  const unit = match?.[2] ? EVERY_UNITS[match[2]] : undefined;
  return match && unit ? `${match[1]} ${unit}${match[1] === "1" ? "" : "s"}` : every;
}

/** The paragraphs of a doc, cut at MAX_DOC_CHARS: blank lines separate them, a single line break (rustfmt wraps `///` blocks) is a space. */
export function docParagraphs(doc: string): string[] {
  const bounded = doc.length > MAX_DOC_CHARS ? `${doc.slice(0, MAX_DOC_CHARS)}…` : doc;
  const paragraphs: string[] = [];
  let lines: string[] = [];
  const flush = () => {
    if (lines.length) paragraphs.push(lines.join(" "));
    lines = [];
  };
  for (const line of bounded.split("\n")) {
    const text = line.trim();
    if (text) lines.push(text);
    else flush();
  }
  flush();
  return paragraphs;
}

/** The first sentence of a doc's first paragraph: it ends at . ! or ? followed by a space and a capital, a backtick or a quote; not after e.g., i.e. or etc., and not inside a number or version. */
export function firstSentence(doc: string): string {
  const first = docParagraphs(doc)[0] ?? "";
  for (const end of first.matchAll(/[.!?](?=\s+[A-Z`"'(])/g)) {
    const word = /[\w.]*$/.exec(first.slice(Math.max(0, end.index - WORD_TAIL), end.index))?.[0] ?? "";
    if (!ABBREVIATIONS.has(word.toLowerCase())) return first.slice(0, end.index + 1);
  }
  return first;
}

/** The first few, then how many more. */
function list(items: string[]): string {
  if (items.length <= LIST_SHOWN) return items.join(", ");
  return `${items.slice(0, LIST_SHOWN).join(", ")}, and ${items.length - LIST_SHOWN} more`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The abi's keys, whether it writes a map or a list of names. */
function namesOf(entries: unknown): string[] {
  if (Array.isArray(entries)) return entries.map((e) => (typeof e === "string" ? e : String((e as { name?: unknown })?.name ?? "")));
  return entries && typeof entries === "object" ? Object.keys(entries) : [];
}

const hasApps = (m: AppAbiMethod) => (m.allowApps?.length ?? 0) > 0;

/** Whether the node's call check can let any caller through: not a setup, timer or compute call, not an app-only call, not a compute result. */
export function reachable(m: AppAbiMethod): boolean {
  return CALLED_KINDS.has(m.kind) && !m.allowAction && !m.computeResult;
}

/** Reachable by someone who is not signed in or is another app: only a call or a view, never an action. */
const plainCall = (m: AppAbiMethod) => reachable(m) && (m.kind === "call" || m.kind === "view");
const publicView = (m: AppAbiMethod) => plainCall(m) && m.public && m.kind === "view";
const publicCall = (m: AppAbiMethod) => plainCall(m) && m.public && m.kind === "call";
const sharedWithApps = (m: AppAbiMethod) => plainCall(m) && hasApps(m); // the node never lets another app start an action
const openToAnyone = (m: AppAbiMethod) => reachable(m) && m.public;

/** Not listed for people: setup, the node's compute callback and calls for the app's own actions, unless one asks for a passkey. */
function internal(m: AppAbiMethod): boolean {
  return (m.kind === "init" || !!m.computeResult || !!m.allowAction) && !m.confirm;
}

const allMethods = (abi: AppAbi | null) => Object.entries(abi?.methods ?? {});

function visibleMethods(abi: AppAbi | null): [string, AppAbiMethod][] {
  return allMethods(abi).filter(([, m]) => !internal(m));
}

/** How many actions the app lists for people. */
export function actionCount(abi: AppAbi | null): number {
  return visibleMethods(abi).length;
}

/** Level 1: the few lines that matter, each only when the card says so; the claim about other apps is qualified when the card is not signed. */
export function summarize(card: ReviewCard, signed = true): SummaryLine[] {
  const lines: SummaryLine[] = [];
  const all = allMethods(card.abi);
  const titles = (pick: (m: AppAbiMethod) => boolean) => all.filter(([, m]) => pick(m)).map(([name]) => humanize(name));

  const passkey = titles((m) => m.confirm);
  if (passkey.length) lines.push({ id: "passkey", title: `Asks for your passkey before ${plural(passkey.length, "sensitive action")}`, detail: list(passkey), tone: "warn" });
  const seen = titles(publicView);
  if (seen.length) lines.push({ id: "public", title: `Shows ${plural(seen.length, "thing")} to anyone`, detail: list(seen), tone: "warn" });
  const changed = titles(publicCall);
  if (changed.length) lines.push({ id: "publicCalls", title: `Lets anyone use ${plural(changed.length, "action")} without signing in`, detail: list(changed), tone: "warn" });
  if (card.outbound.length) lines.push({ id: "outside", title: `Contacts ${plural(card.outbound.length, "outside service")}`, detail: list(card.outbound.map(hostOf)) });
  const timers = all.filter(([, m]) => m.kind === "timer").map(([name, m]) => `${m.every ? `Every ${everyWords(m.every)}: ` : ""}${humanize(name)}`);
  if (timers.length) lines.push({ id: "timers", title: `Runs ${plural(timers.length, "task")} on its own`, detail: list(timers) });

  if (card.abi) {
    const methods = Object.values(card.abi.methods);
    const shared = methods.filter(sharedWithApps);
    if (shared.length > 0) {
      const apps = [...new Set(shared.flatMap((m) => m.allowApps ?? []))];
      lines.push({ id: "apps", title: `Lets other apps call ${plural(shared.length, "action")}`, detail: apps.includes("*") ? "Any app" : list(apps), tone: "warn" });
    } else if (!methods.some(openToAnyone)) {
      lines.push(
        signed
          ? { id: "apps", title: "No other app can use your data", detail: "No action is open to other apps", tone: "ok" }
          : { id: "apps", title: "No other app can use your data, as this test server lists it", detail: "No action is open to other apps" },
      );
    }
  }
  return lines;
}

function whoCanTrigger(m: AppAbiMethod): string | null {
  if (m.kind === "timer") return "The system, on a schedule";
  if (m.allowAction) return "Only the app's own actions";
  if (m.kind === "action") return "You, signed in to this app";
  if (m.kind === "compute") return "The app itself";
  if (m.kind !== "call" && m.kind !== "view") return null;
  const who = [m.public ? "Anyone, no sign-in" : "You, signed in to this app"];
  if (m.allowSystem) who.push("the system");
  if (hasApps(m)) who.push(`apps: ${(m.allowApps ?? []).map((a) => (a === "*" ? "any app" : a)).join(", ")}`);
  return who.join("; ");
}

const KIND_LABEL: Record<string, string> = {
  call: "Change (call)",
  view: "Read (view)",
  timer: "Timer",
  action: "Action",
  compute: "Compute",
};

function methodTags(m: AppAbiMethod): string[] {
  const tags: string[] = [];
  if (m.confirm) tags.push("passkey");
  if (plainCall(m) && m.public) tags.push("public");
  if (m.kind === "call" || m.kind === "action") tags.push("changes data");
  else if (m.kind === "view") tags.push("reads data");
  else tags.push(m.kind);
  return tags;
}

function methodFacts(name: string, m: AppAbiMethod): ActionFact[] {
  const facts: ActionFact[] = [
    { label: "Technical name", value: name, mono: true },
    { label: "Type", value: KIND_LABEL[m.kind] ?? m.kind },
  ];
  if (m.kind === "timer" && m.every) facts.push({ label: "Runs", value: `Every ${everyWords(m.every)}` });
  const who = whoCanTrigger(m);
  if (who) facts.push({ label: "Who can trigger", value: who });
  facts.push(
    { label: "Confirmation", value: m.confirm ? "Passkey prompt every time" : "None" },
    { label: "Takes", value: m.params.length ? m.params.map(([n, t]) => `${n}: ${t}`).join(", ") : "Nothing", mono: m.params.length > 0 },
    { label: "Returns", value: m.returns ?? "Nothing", mono: m.returns !== null },
  );
  if (m.kind !== "timer") facts.push({ label: "Limit", value: m.rate ?? "None" });
  return facts;
}

function methodView(name: string, m: AppAbiMethod): ActionView {
  const paragraphs = docParagraphs(m.doc ?? "");
  const doc = paragraphs.length ? { doc: paragraphs.join("\n\n"), summary: firstSentence(m.doc ?? "") } : {};
  return { name, title: humanize(name), ...doc, tags: methodTags(m), facts: methodFacts(name, m) };
}

const MEMBERS: Array<Omit<ActionGroup, "items"> & { has: (m: AppAbiMethod) => boolean }> = [
  { id: "passkey", label: "Needs your passkey", chip: "Needs passkey", blurb: "The app cannot do these without you confirming", has: (m) => m.confirm },
  { id: "public", label: "Visible to anyone", chip: "Public", blurb: "Can be used without signing in", has: (m) => plainCall(m) && m.public },
  { id: "changes", label: "Changes your data", chip: "Changes data", blurb: "Create, update or delete what is stored", has: (m) => m.kind === "call" || m.kind === "action" },
  { id: "reads", label: "Reads your data", chip: "Reads data", blurb: "Look at what is stored without changing it", has: (m) => m.kind === "view" },
  { id: "timers", label: "Runs on its own", chip: "On its own", blurb: "Starts without anyone asking, on a schedule", has: (m) => m.kind === "timer" },
];
const OTHER_KINDS = new Set(["call", "action", "view", "timer"]);

/** Level 2: the actions in plain-word groups. An action sits in every group it belongs to; a group with nothing in it is left out. */
export function actionGroups(card: ReviewCard): ActionGroup[] {
  const visible = visibleMethods(card.abi);
  const groups: ActionGroup[] = MEMBERS.map(({ has, ...group }) => ({
    ...group,
    items: visible.filter(([, m]) => has(m)).map(([name, m]) => methodView(name, m)),
  }));
  groups.push({
    id: "outside",
    label: "Outside services",
    chip: "Outside",
    blurb: "The only sites the app may contact, enforced by the node",
    items: card.outbound.map((host) => ({ name: host, title: hostOf(host), tags: [], facts: [] })),
  });
  const events = card.abi?.events;
  groups.push({
    id: "events",
    label: "Updates it broadcasts",
    chip: "Updates",
    blurb: "Messages the app sends out when something happens",
    items: namesOf(events).map((name) => {
      const fields = events && !Array.isArray(events) ? (events as Record<string, unknown>)[name] : null;
      const facts = fields && typeof fields === "object" ? Object.entries(fields).map(([label, type]) => ({ label, value: String(type), mono: true })) : [];
      return { name, title: humanize(name), tags: [], facts };
    }),
  });
  groups.push({
    id: "other",
    label: "Other methods",
    chip: "Other",
    blurb: "Do not fit the groups above",
    items: visible.filter(([, m]) => !OTHER_KINDS.has(m.kind)).map(([name, m]) => methodView(name, m)),
  });
  return groups.filter((g) => g.items.length > 0);
}

/** Amber lines for what leaves the sealed boundary: each public view, each public call and each outside host. A test server has no sealed hardware to leave. */
export function exceptions(card: ReviewCard, sealed = true): string[] {
  const named = (pick: (m: AppAbiMethod) => boolean, prefix: string) => allMethods(card.abi).filter(([, m]) => pick(m)).map(([name]) => `${prefix}${humanize(name)}`);
  const hosts = card.outbound.map((host) => `${sealed ? "Leaves the sealed hardware" : "Contacts"}: ${hostOf(host)}`);
  return [...named(publicView, "Anyone can see: "), ...named(publicCall, "Anyone can change: "), ...hosts];
}

/** Level 3: the signed hashes, in full. */
export function rawFacts(card: PackageCapabilities): ActionFact[] {
  return [
    { label: "Manifest hash", value: card.manifestHash },
    { label: "WASM (bundle) hash", value: card.bundle },
    { label: "Signer", value: card.signer },
    { label: ".mpk hash", value: card.mpk },
  ];
}

export const abiFileName = (card: Pick<PackageCapabilities, "package" | "version">) => `${card.package}-${card.version}-abi.json`;

/** `did:key:z6Mk…2doK`. */
export function shortSigner(signer: string): string {
  return signer.length > SIGNER_SHORT_AFTER ? `${signer.slice(0, SIGNER_HEAD)}…${signer.slice(-SIGNER_TAIL)}` : signer;
}

/** What the node says about the app's code: its check of an instance, `"pending"` while that runs, or `null` before an instance exists. */
export type CodeEvidence = AppCodeCheck | "pending" | null;

export interface CodeLine {
  text: string;
  tone?: "good" | "bad";
  note?: string;
  /** The node runs another release than the card's, or the card's release with other code. */
  divergence?: "release" | "code";
}

const norm = (hash: string) => hash.trim().toLowerCase();

/** The code hash line. Only a node's match for this card's own release says "attested"; `signed` is false when a test server lists the card. */
export function codeLine(card: Pick<PackageCapabilities, "bundle" | "package" | "version">, evidence: CodeEvidence, signed = true): CodeLine {
  const fromRelease = `bundle ${shortHash(card.bundle)}, from ${signed ? "the signed release" : "the release as this test server lists it"}`;
  if (evidence === null) return { text: fromRelease };
  if (evidence === "pending") return { text: fromRelease, note: "Checking that this node runs it" };
  switch (evidence.status) {
    case "match": {
      const pkg = evidence.package ?? card.package;
      const version = evidence.version ?? card.version;
      if (pkg !== card.package || version !== card.version || norm(evidence.appHash) !== norm(card.bundle)) {
        const text = `The node reports different code: it runs ${pkg} v${version} (bundle ${shortHash(evidence.appHash)})`;
        return { text, tone: "bad", divergence: pkg !== card.package || version !== card.version ? "release" : "code" };
      }
      return {
        text: `bundle ${shortHash(evidence.appHash)}, attested by this node`,
        tone: "good",
        ...(evidence.package && evidence.version ? { note: `Node runs the code listed for ${evidence.package} v${evidence.version}` } : {}),
      };
    }
    case "mismatch":
      return { text: `Does not match the signed release: the node runs bundle ${shortHash(evidence.appHash)}`, tone: "bad", divergence: "code" };
    case "unknown":
      return { text: fromRelease, note: `This node could not confirm it: ${evidence.reason}` };
  }
}

/** The claims about the data: control and who can open it hold for a node that passed its check (as that server's own on a test server), "Calimero can't read it" only for a sealed one. `member` is a workspace that exists, seen by someone who may have joined later. */
export function dataStatements(kind: ShieldKind, viewer: "creator" | "member", noun = "workspace"): string[] {
  const CONTROL = control(viewer, noun);
  if (kind === "verified") return [`${CONTROL}.`, `${INVITED_ONLY}.`, NOT_READABLE];
  if (kind === "unsealed") return [`${CONTROL}${TEST_SERVER_ENFORCES}`, `${INVITED_ONLY}${TEST_SERVER_ENFORCES}`];
  return [];
}
