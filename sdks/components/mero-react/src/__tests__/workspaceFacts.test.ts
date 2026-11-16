import { describe, expect, it } from "vitest";
import type { AppAbi, AppAbiMethod, PackageCapabilities } from "@calimero-network/mero-js";

import {
  abiFileName,
  actionCount,
  actionGroups,
  codeLine,
  dataStatements,
  docParagraphs,
  exceptions,
  firstSentence,
  humanize,
  MAX_DOC_CHARS,
  reachable,
  rawFacts,
  shortSigner,
  summarize,
  type ActionGroup,
} from "../create-workspace/workspaceFacts";

const BUNDLE = "7f3a".padEnd(60, "0") + "c91e";
const NODE_HASH = "aa11".padEnd(60, "0") + "bb22";

const method = (extra: Partial<AppAbiMethod> = {}): AppAbiMethod => ({
  kind: "call",
  params: [],
  returns: null,
  confirm: false,
  public: false,
  allowSystem: false,
  ...extra,
});

const abi = (methods: Record<string, AppAbiMethod>, events: unknown = {}): AppAbi => ({
  abiVersion: "1",
  world: "calimero:app@0.5.1",
  state: "",
  methods,
  errors: [],
  events: events as unknown[],
});

const caps = (over: Partial<PackageCapabilities> = {}): PackageCapabilities => ({
  package: "network.calimero.crm",
  version: "1.4.0",
  service: null,
  bundle: BUNDLE,
  mpk: "mpk-hash",
  manifestHash: "manifest-hash",
  signer: "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
  name: "Mero CRM",
  author: null,
  outbound: [],
  abi: abi({}),
  ...over,
});

const crm = caps({
  outbound: ["https://api.stripe.com/v1/"],
  abi: abi(
    {
      init: method({ kind: "init" }),
      deleteWorkspace: method({ confirm: true, params: [["workspaceId", "string"]] }),
      removeMember: method({ confirm: true }),
      exportAll: method({ kind: "view", confirm: true }),
      pipelineSummary: method({ kind: "view", public: true, rate: "10/s", returns: "list<Stage>", doc: "Deal counts per stage." }),
      createDeal: method(),
      listDeals: method({ kind: "view" }),
      sendReminders: method({ kind: "timer", every: "1h", allowSystem: true }),
      internalStep: method({ allowAction: true }),
      receiveResult: method({ computeResult: true, allowSystem: true }),
    },
    { DealMoved: { id: "string" }, NoteAdded: {} },
  ),
});

const group = (groups: ActionGroup[], id: string) => groups.find((g) => g.id === id);
const line = (c: PackageCapabilities, id: string) => summarize(c).find((l) => l.id === id);

describe("humanize", () => {
  it.each([
    ["deleteWorkspace", "Delete workspace"],
    ["delete_workspace", "Delete workspace"],
    ["pipelineSummary", "Pipeline summary"],
    ["exportAll", "Export all"],
    ["x", "X"],
    ["", ""],
  ])("%s -> %s", (name, shown) => expect(humanize(name)).toBe(shown));
});

const ISSUE_DOC =
  "Moves an issue to another column of the board. `status` is exactly\n`Open`, `In progress`, `Blocked` or `Done` (else `Invalid`). `NotFound`\nfor an unknown issue. Emits `IssueStatusChanged`.";

describe("docParagraphs", () => {
  it("joins the lines of a paragraph and keeps blank-line breaks", () => {
    expect(docParagraphs("Moves it\nacross.\n\nSecond\n  paragraph.\n \n\nThird.")).toEqual(["Moves it across.", "Second paragraph.", "Third."]);
  });

  it("is empty for blank text", () => {
    expect(docParagraphs(" \n\n ")).toEqual([]);
  });

  it("treats CRLF line breaks like LF", () => {
    expect(docParagraphs("One\r\ntwo.\r\n\r\nThree.")).toEqual(["One two.", "Three."]);
  });
});

describe("a publisher's description of any length", () => {
  const hostile = ["a.".repeat(50_000), "e.g. E.g. ".repeat(10_000), " ".repeat(100_000), "x".repeat(100_000)];

  it("is cut at the limit with an ellipsis before anything is parsed", () => {
    for (const doc of hostile) {
      const [paragraph] = docParagraphs(doc);
      expect(paragraph?.length ?? 0).toBeLessThanOrEqual(MAX_DOC_CHARS + 1);
    }
    expect(docParagraphs("a. ".repeat(10_000))[0]?.endsWith("…")).toBe(true);
    expect(docParagraphs("Short.")).toEqual(["Short."]);
  });

  it("is split within a small time budget", () => {
    const started = performance.now();
    for (const doc of hostile) firstSentence(doc);
    expect(performance.now() - started).toBeLessThan(250);
  });

  it("limits what the row and the expanded facts show", () => {
    const item = group(actionGroups(caps({ abi: abi({ big: method({ doc: "e.g. E.g. ".repeat(10_000) }) }) })), "changes")?.items[0];
    expect(item?.doc?.length).toBeLessThanOrEqual(MAX_DOC_CHARS + 1);
    expect(item?.summary?.length).toBeLessThanOrEqual(MAX_DOC_CHARS + 1);
  });
});

describe("firstSentence", () => {
  it("is the first sentence of the real doc, hard line breaks and all", () => {
    expect(firstSentence(ISSUE_DOC)).toBe("Moves an issue to another column of the board.");
  });

  it.each([
    ["Sends a receipt (e.g. by mail) when paid. Emits `Paid`.", "Sends a receipt (e.g. by mail) when paid."],
    ["Needs v1.2.3 or later, i.e. the new API. Else `Forbidden`.", "Needs v1.2.3 or later, i.e. the new API."],
    ["Closes the deal! Emits `DealClosed`.", "Closes the deal!"],
    ["Is it open? `NotFound` for none.", "Is it open?"],
    ["Lists deals, notes etc. Newest first.", "Lists deals, notes etc. Newest first."],
    ["Pays 3.5 percent. More.", "Pays 3.5 percent."],
    ["No full stop here", "No full stop here"],
    ["Only the first paragraph\n\nSecond one. Third.", "Only the first paragraph"],
    ["One. Two.", "One."],
  ])("%j -> %j", (doc, first) => expect(firstSentence(doc)).toBe(first));
});

describe("summarize", () => {
  it("says what matters, each line only when the card backs it", () => {
    const lines = summarize(crm);
    expect(lines.map((l) => l.id)).toEqual(["passkey", "public", "outside", "timers"]);
    expect(line(crm, "passkey")?.title).toBe("Asks for your passkey before 3 sensitive actions");
    expect(line(crm, "passkey")?.detail).toBe("Delete workspace, Remove member, Export all");
    expect(line(crm, "public")?.title).toBe("Shows 1 thing to anyone");
    expect(line(crm, "outside")?.detail).toBe("api.stripe.com");
    expect(line(crm, "timers")?.title).toBe("Runs 1 task on its own");
    expect(line(crm, "timers")?.detail).toBe("Every 1 hour: Send reminders");
  });

  it("omits every line the card does not back", () => {
    const bare = caps({ abi: abi({ createDeal: method(), listDeals: method({ kind: "view" }) }) });
    expect(summarize(bare).map((l) => l.id)).toEqual(["apps"]);
  });

  it("claims no other app can use the data only when no reachable method allows an app or is public", () => {
    const closed = caps({ abi: abi({ createDeal: method(), listDeals: method({ kind: "view" }) }) });
    expect(line(closed, "apps")?.title).toBe("No other app can use your data");
    expect(line(closed, "apps")?.tone).toBe("ok");
    const listed = summarize(closed, false).find((l) => l.id === "apps");
    expect(listed?.title).toBe("No other app can use your data, as this test server lists it");
    expect(listed?.tone).toBeUndefined();
    expect(line(crm, "apps")).toBeUndefined();
    const anyone = caps({ abi: abi({ ping: method({ public: true }) }) });
    expect(line(anyone, "apps")).toBeUndefined();
    const shared = caps({ abi: abi({ onPayment: method({ allowApps: ["billing"] }), other: method({ allowApps: [] }) }) });
    expect(line(shared, "apps")?.title).toBe("Lets other apps call 1 action");
    expect(line(shared, "apps")?.detail).toBe("billing");
    expect(line(shared, "apps")?.tone).toBe("warn");
  });

  it("reads * as any app", () => {
    const open = caps({ abi: abi({ onPayment: method({ allowApps: ["*"] }) }) });
    expect(line(open, "apps")?.title).toBe("Lets other apps call 1 action");
    expect(line(open, "apps")?.detail).toBe("Any app");
  });

  it("counts an app allowed on a call or a view, never on an action the node does not let an app start, and never claims none when one is public", () => {
    const act = caps({ abi: abi({ approve: method({ kind: "action", allowApps: ["billing"] }), onPayment: method({ kind: "view", allowApps: ["billing"] }) }) });
    expect(line(act, "apps")?.title).toBe("Lets other apps call 1 action");
    const onlyAction = caps({ abi: abi({ approve: method({ kind: "action", allowApps: ["billing"] }) }) });
    expect(line(onlyAction, "apps")?.title).toBe("No other app can use your data");
    const publicAction = caps({ abi: abi({ approve: method({ kind: "action", public: true }) }) });
    expect(line(publicAction, "apps")).toBeUndefined();
  });

  it("ignores an app allowed on a method no outside caller can reach", () => {
    const hidden = caps({ abi: abi({ step: method({ allowAction: true, allowApps: ["*"] }), result: method({ computeResult: true, allowApps: ["*"] }) }) });
    expect(line(hidden, "apps")?.title).toBe("No other app can use your data");
  });

  it("splits what anyone can see from what anyone can do", () => {
    const open = caps({ abi: abi({ listDeals: method({ kind: "view", public: true }), signUp: method({ public: true }), joinWaitlist: method({ public: true }) }) });
    expect(line(open, "public")?.title).toBe("Shows 1 thing to anyone");
    expect(line(open, "public")?.detail).toBe("List deals");
    const calls = line(open, "publicCalls");
    expect(calls?.title).toBe("Lets anyone use 2 actions without signing in");
    expect(calls?.detail).toBe("Sign up, Join waitlist");
    expect(calls?.tone).toBe("warn");
  });

  it("does not call a public method anyone when nobody outside can reach it", () => {
    const none = caps({ abi: abi({ init: method({ kind: "init", public: true }), step: method({ allowAction: true, public: true }), result: method({ kind: "view", computeResult: true, public: true }), tick: method({ kind: "timer", public: true }) }) });
    expect(line(none, "public")).toBeUndefined();
    expect(line(none, "publicCalls")).toBeUndefined();
  });

  it("makes no claim about the app's actions when it publishes no action list", () => {
    const none = caps({ abi: null, outbound: ["api.example.com"] });
    expect(summarize(none).map((l) => l.id)).toEqual(["outside"]);
  });

  it("shortens a long list of names", () => {
    const many = caps({ abi: abi(Object.fromEntries(["a", "b", "c", "d", "e"].map((n) => [n, method({ confirm: true })]))) });
    expect(line(many, "passkey")?.detail).toBe("A, B, C, and 2 more");
  });
});

describe("actionGroups", () => {
  const groups = actionGroups(crm);

  it("hides init, computeResult and allowAction-only methods from every group", () => {
    const titles = groups.flatMap((g) => g.items.map((i) => i.name));
    expect(titles).not.toContain("init");
    expect(titles).not.toContain("receiveResult");
    expect(titles).not.toContain("internalStep");
    expect(actionCount(crm.abi)).toBe(7);
  });

  it("groups in plain words, an action in every group it belongs to", () => {
    expect(groups.map((g) => g.label)).toEqual([
      "Needs your passkey",
      "Visible to anyone",
      "Changes your data",
      "Reads your data",
      "Runs on its own",
      "Outside services",
      "Updates it broadcasts",
    ]);
    expect(group(groups, "passkey")?.items.map((i) => i.title)).toEqual(["Delete workspace", "Remove member", "Export all"]);
    expect(group(groups, "changes")?.items.map((i) => i.name)).toEqual(["deleteWorkspace", "removeMember", "createDeal"]);
    expect(group(groups, "reads")?.items.map((i) => i.name)).toEqual(["exportAll", "pipelineSummary", "listDeals"]);
    expect(group(groups, "public")?.items.map((i) => i.name)).toEqual(["pipelineSummary"]);
    expect(group(groups, "timers")?.items.map((i) => i.name)).toEqual(["sendReminders"]);
  });

  it("lists outside services as enforced by the node and events by name", () => {
    expect(group(groups, "outside")?.items.map((i) => i.title)).toEqual(["api.stripe.com"]);
    expect(group(groups, "outside")?.blurb).toContain("enforced by the node");
    expect(group(groups, "events")?.items.map((i) => i.title)).toEqual(["Deal moved", "Note added"]);
  });

  it("leaves out a group with nothing in it", () => {
    const small = actionGroups(caps({ abi: abi({ createDeal: method() }) }));
    expect(small.map((g) => g.id)).toEqual(["changes"]);
  });

  it("puts a kind that fits no group under other methods", () => {
    const odd = actionGroups(caps({ abi: abi({ crunch: method({ kind: "compute" }) }) }));
    expect(odd.map((g) => g.id)).toEqual(["other"]);
  });

  it("states the technical facts of an action", () => {
    const del = group(groups, "passkey")?.items[0];
    const facts = Object.fromEntries((del?.facts ?? []).map((f) => [f.label, f.value]));
    expect(facts).toEqual({
      "Technical name": "deleteWorkspace",
      Type: "Change (call)",
      "Who can trigger": "You, signed in to this app",
      Confirmation: "Passkey prompt every time",
      Takes: "workspaceId: string",
      Returns: "Nothing",
      Limit: "None",
    });
    expect(del?.tags).toEqual(["passkey", "changes data"]);
  });

  it("says who can trigger it from public, system, apps and allowAction", () => {
    const who = (m: AppAbiMethod) => {
      const g = actionGroups(caps({ abi: abi({ m }) }));
      return g[0]?.items[0]?.facts.find((f) => f.label === "Who can trigger")?.value;
    };
    expect(who(method({ public: true }))).toBe("Anyone, no sign-in");
    expect(who(method({ allowSystem: true }))).toBe("You, signed in to this app; the system");
    expect(who(method({ allowApps: ["billing", "*"] }))).toBe("You, signed in to this app; apps: billing, any app");
    expect(who(method({ allowAction: true, confirm: true, public: true, allowSystem: true, allowApps: ["*"] }))).toBe("Only the app's own actions");
    expect(who(method({ kind: "action" }))).toBe("You, signed in to this app");
    expect(who(method({ kind: "compute" }))).toBe("The app itself");
  });

  it("tags and groups a method public only when a caller without a session can reach it", () => {
    const c = caps({ abi: abi({ ping: method({ public: true }), act: method({ kind: "action", public: true }), step: method({ allowAction: true, public: true, confirm: true }) }) });
    const g = actionGroups(c);
    expect(group(g, "public")?.items.map((i) => i.name)).toEqual(["ping"]);
    expect(group(g, "changes")?.items.find((i) => i.name === "act")?.tags).toEqual(["changes data"]);
    expect(group(g, "passkey")?.items.find((i) => i.name === "step")?.tags).toEqual(["passkey", "changes data"]);
  });

  it("shows the limit of a public view and the interval of a timer", () => {
    const pub = group(groups, "public")?.items[0]?.facts.find((f) => f.label === "Limit");
    expect(pub?.value).toBe("10/s");
    const timer = group(groups, "timers")?.items[0]?.facts.find((f) => f.label === "Runs");
    expect(timer?.value).toBe("Every 1 hour");
  });

  it("keeps the whole description on one line per paragraph and its first sentence apart", () => {
    const c = caps({ abi: abi({ moveIssue: method({ doc: ISSUE_DOC }) }) });
    const item = group(actionGroups(c), "changes")?.items[0];
    expect(item?.doc).toBe(
      "Moves an issue to another column of the board. `status` is exactly `Open`, `In progress`, `Blocked` or `Done` (else `Invalid`). `NotFound` for an unknown issue. Emits `IssueStatusChanged`.",
    );
    expect(item?.summary).toBe("Moves an issue to another column of the board.");
    expect(group(actionGroups(caps({ abi: abi({ plain: method() }) })), "changes")?.items[0]?.summary).toBeUndefined();
  });

  it("keeps the publisher's description apart from the facts", () => {
    const view = group(groups, "public")?.items[0];
    expect(view?.doc).toBe("Deal counts per stage.");
    expect(view?.facts.some((f) => f.value.includes("Deal counts"))).toBe(false);
    expect(group(groups, "changes")?.items[0]?.doc).toBeUndefined();
  });

  it("is empty with no action list", () => {
    expect(actionGroups(caps({ abi: null, outbound: [] }))).toEqual([]);
    expect(actionCount(null)).toBe(0);
  });
});

describe("reachable", () => {
  it.each([
    [method(), true],
    [method({ kind: "view", public: true }), true],
    [method({ kind: "action" }), true],
    [method({ allowAction: true, public: true, allowSystem: true, allowApps: ["*"] }), false],
    [method({ computeResult: true, allowSystem: true }), false],
    [method({ kind: "init", public: true }), false],
    [method({ kind: "timer", public: true }), false],
  ])("%j -> %s", (m, expected) => expect(reachable(m)).toBe(expected));
});

describe("a publisher-controlled abi", () => {
  const odd = caps({
    abi: abi({
      init: method({ kind: "init", public: true }),
      confirmedInit: method({ kind: "init", confirm: true }),
      receiveResult: method({ kind: "view", computeResult: true, allowApps: ["billing"], public: true }),
      stepOne: method({ allowAction: true, public: true }),
      stepTwo: method({ allowAction: true, allowSystem: true, allowApps: ["*"] }),
      stepConfirm: method({ allowAction: true, confirm: true }),
      quiet: method({ allowAction: true }),
    }),
  });
  const shown = () => actionGroups(odd).flatMap((g) => g.items.map((i) => i.name));

  it("makes no outward claim for a method abi_check lets no outside caller through", () => {
    expect(line(odd, "public")).toBeUndefined();
    expect(line(odd, "publicCalls")).toBeUndefined();
    expect(line(odd, "apps")?.title).toBe("No other app can use your data");
    expect(exceptions(odd)).toEqual([]);
  });

  it("still counts a passkey method it hides nothing about", () => {
    expect(line(odd, "passkey")?.title).toBe("Asks for your passkey before 2 sensitive actions");
    expect(shown()).toEqual(expect.arrayContaining(["confirmedInit", "stepConfirm"]));
  });

  it("lists no method that nothing outside can reach and that asks for no passkey", () => {
    for (const name of ["init", "receiveResult", "stepOne", "stepTwo", "quiet"]) expect(shown()).not.toContain(name);
    expect(actionCount(odd.abi)).toBe(2);
  });
});

describe("exceptions", () => {
  it("names each public view, each public call and each outbound host", () => {
    expect(exceptions(crm)).toEqual(["Anyone can see: Pipeline summary", "Leaves the sealed hardware: api.stripe.com"]);
    const c = caps({ abi: abi({ ping: method({ public: true }), listDeals: method({ kind: "view" }), openDeals: method({ kind: "view", public: true }) }) });
    expect(exceptions(c)).toEqual(["Anyone can see: Open deals", "Anyone can change: Ping"]);
  });

  it("does not name a private view", () => {
    expect(exceptions(caps({ abi: abi({ listDeals: method({ kind: "view" }) }) }))).toEqual([]);
  });

  it("lists several hosts one per line", () => {
    const c = caps({ outbound: ["a.example.com", "https://b.example.com/x"] });
    expect(exceptions(c)).toEqual(["Leaves the sealed hardware: a.example.com", "Leaves the sealed hardware: b.example.com"]);
  });

  it("does not speak of sealed hardware for a test server", () => {
    expect(exceptions(crm, false)).toEqual(["Anyone can see: Pipeline summary", "Contacts: api.stripe.com"]);
  });
});

describe("rawFacts", () => {
  it("lists the signed hashes in full", () => {
    expect(rawFacts(crm)).toEqual([
      { label: "Manifest hash", value: "manifest-hash" },
      { label: "WASM (bundle) hash", value: BUNDLE },
      { label: "Signer", value: crm.signer },
      { label: ".mpk hash", value: "mpk-hash" },
    ]);
  });

  it("names the ABI file after the package release", () => {
    expect(abiFileName(crm)).toBe("network.calimero.crm-1.4.0-abi.json");
  });
});

describe("codeLine", () => {
  const listed = { bundle: BUNDLE, package: "network.calimero.crm", version: "1.4.0" };

  it("before creating, comes from the signed release and is never attested", () => {
    const shown = codeLine(listed, null);
    expect(shown.text).toBe("bundle 7f3a…c91e, from the signed release");
    expect(shown.text).not.toContain("attested");
  });

  it("says a test server lists the release when the node is not sealed", () => {
    expect(codeLine(listed, null, false).text).toBe("bundle 7f3a…c91e, from the release as this test server lists it");
    const unknown = codeLine(listed, { status: "unknown", reason: "mock" }, false);
    expect(unknown.text).toBe("bundle 7f3a…c91e, from the release as this test server lists it");
  });

  it("is attested by the node only on a match, with the hash the node bound", () => {
    const shown = codeLine(listed, { status: "match", appHash: BUNDLE, checkedAt: 1 });
    expect(shown.text).toBe("bundle 7f3a…c91e, attested by this node");
    expect(shown.tone).toBe("good");
    expect(shown.note).toBeUndefined();
  });

  it("says which release the node's code was checked against when the match carries it", () => {
    const shown = codeLine(listed, { status: "match", appHash: BUNDLE, checkedAt: 1, package: "network.calimero.crm", version: "1.4.0" });
    expect(shown.text).toBe("bundle 7f3a…c91e, attested by this node");
    expect(shown.note).toBe("Node runs the code listed for network.calimero.crm v1.4.0");
  });

  it("does not attest a card of another release than the one the node's instance runs", () => {
    const shown = codeLine(listed, { status: "match", appHash: BUNDLE, checkedAt: 1, package: "network.calimero.crm", version: "1.3.0" });
    expect(shown.tone).toBe("bad");
    expect(shown.text).toBe("The node reports different code: it runs network.calimero.crm v1.3.0 (bundle 7f3a…c91e)");
    expect(shown.divergence).toBe("release");
    const other = codeLine(listed, { status: "match", appHash: BUNDLE, checkedAt: 1, package: "network.calimero.other", version: "1.4.0" });
    expect(other.tone).toBe("bad");
    expect(other.divergence).toBe("release");
  });

  it("does not attest a match whose hash is not the card's", () => {
    const shown = codeLine(listed, { status: "match", appHash: NODE_HASH, checkedAt: 1, package: "network.calimero.crm", version: "1.4.0" });
    expect(shown.tone).toBe("bad");
    expect(shown.text).not.toContain("attested");
    expect(shown.divergence).toBe("code");
  });

  it("flags a mismatch and says what the node runs", () => {
    const shown = codeLine(listed, { status: "mismatch", appHash: NODE_HASH, expected: BUNDLE });
    expect(shown.tone).toBe("bad");
    expect(shown.text).toBe("Does not match the signed release: the node runs bundle aa11…bb22");
    expect(shown.divergence).toBe("code");
  });

  it("falls back to the signed release when the node could not check", () => {
    const shown = codeLine(listed, { status: "unknown", reason: "test server: the node's quote is a mock" });
    expect(shown.text).toBe("bundle 7f3a…c91e, from the signed release");
    expect(shown.note).toBe("This node could not confirm it: test server: the node's quote is a mock");
  });

  it("says it is checking while the node's answer is pending", () => {
    const shown = codeLine(listed, "pending");
    expect(shown.text).toBe("bundle 7f3a…c91e, from the signed release");
    expect(shown.note).toBe("Checking that this node runs it");
  });
});

describe("dataStatements", () => {
  it("names what the owners control after the noun", () => {
    expect(dataStatements("verified", "member", "lobby")[0]).toBe("Only the lobby's owners control it.");
    expect(dataStatements("verified", "member")[0]).toBe("Only the workspace's owners control it.");
  });

  it("makes the org claims for a node that passed its check, and the third only for a sealed one", () => {
    expect(dataStatements("verified", "creator")).toEqual(["Only you control it.", "Only people invited to it can open it.", "Calimero can't read it."]);
    for (const kind of ["checking", "refused"] as const) expect(dataStatements(kind, "creator")).toEqual([]);
  });

  it("words the org claims as the test server's own for an unsealed node, so nothing there reads as verified", () => {
    expect(dataStatements("unsealed", "creator")).toEqual(["Only you control it, as this test server enforces it", "Only people invited to it can open it, as this test server enforces it"]);
    expect(dataStatements("unsealed", "member")[0]).toBe("Only the workspace's owners control it, as this test server enforces it");
  });

  it("says the owners control a workspace that exists, as the viewer may be a later member", () => {
    expect(dataStatements("verified", "member")).toEqual(["Only the workspace's owners control it.", "Only people invited to it can open it.", "Calimero can't read it."]);
  });
});

describe("shortSigner", () => {
  it("keeps the did prefix and the ends", () => {
    expect(shortSigner("did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK")).toBe("did:key:z6Mk…2doK");
    expect(shortSigner("did:key:z6Mk")).toBe("did:key:z6Mk");
  });
});
