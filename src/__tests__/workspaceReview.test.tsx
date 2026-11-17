import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { verifyNodeUrl, type AppAbiMethod, type CalimeroClient, type NodeVerificationState, type PackageCapabilities } from "@calimero-network/mero-js";

import { WorkspaceReview } from "../create-workspace/WorkspaceReview";

const NODE = "https://node-3.relay.example.net";
const BUNDLE = "7f3a".padEnd(60, "0") + "c91e";
const RELEASE = { package: "network.calimero.crm", version: "1.4.0" };

const sealed: NodeVerificationState = { status: "verified", url: NODE, mock: false, release: "2.3.148", profile: "locked-read-only", checkedAt: Date.now() };
const mock: NodeVerificationState = { status: "verified", url: "http://127.0.0.1:2528", mock: true, release: "2.3.148", profile: "locked-read-only", checkedAt: Date.now() };

const method = (extra: Partial<AppAbiMethod> = {}): AppAbiMethod => ({ kind: "call", params: [], returns: null, confirm: false, public: false, allowSystem: false, ...extra });

const card = (over: Partial<PackageCapabilities> = {}): PackageCapabilities => ({
  ...RELEASE,
  service: null,
  bundle: BUNDLE,
  mpk: "mpk-hash",
  manifestHash: "manifest-hash",
  signer: "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
  name: "Mero CRM",
  author: null,
  outbound: ["https://api.stripe.com/v1/"],
  abi: {
    abiVersion: "1",
    world: "calimero:app@0.5.1",
    state: "",
    errors: [],
    events: { DealMoved: { id: "string" } } as unknown as unknown[],
    methods: {
      init: method({ kind: "init" }),
      deleteWorkspace: method({ confirm: true, params: [["workspaceId", "string"]], doc: "Removes the workspace for good." }),
      pipelineSummary: method({ kind: "view", public: true, rate: "10/s", returns: "list<Stage>" }),
      createDeal: method(),
      sendReminders: method({ kind: "timer", every: "1h", allowSystem: true }),
    },
  },
  ...over,
});

const closed = () => card({ abi: { ...card().abi!, methods: { deleteWorkspace: method({ confirm: true }), createDeal: method() } } });
const CHECK_PATH = "m5 12.5 4.5 4.5L19 7.5";

function fakeClient(
  state: NodeVerificationState,
  loaded: () => Promise<PackageCapabilities>,
  attested: { appHash: string; mock?: boolean } = { appHash: BUNDLE },
  verify: () => Promise<NodeVerificationState> = async () => state,
) {
  const packageCapabilities = vi.fn(loaded);
  const attestApp = vi.fn(async () => ({ url: NODE, mock: false, checkedAt: 1, ...attested }));
  const client = {
    nodeUrl: state.url,
    nodes: [NODE, "https://node-4.relay.example.net", "https://node-5.relay.example.net"],
    verification: vi.fn(() => state),
    verify: vi.fn(verify),
    packageCapabilities,
    attestApp,
  };
  return { client: client as unknown as CalimeroClient, packageCapabilities, attestApp };
}

let current: { client: CalimeroClient; cluster: string; local: boolean } | null = null;
function connect(
  state: NodeVerificationState,
  loaded: () => Promise<PackageCapabilities> = async () => card(),
  attested?: { appHash: string; mock?: boolean },
  verify?: () => Promise<NodeVerificationState>,
  cluster = "Calimero Cloud eu-shared-2",
  local = false,
) {
  const fake = fakeClient(state, loaded, attested, verify);
  current = { client: fake.client, cluster, local };
  return fake;
}

const review = (props: { creating?: boolean } = {}) => {
  const onBack = vi.fn();
  const onCreate = vi.fn();
  render(<WorkspaceReview client={current?.client ?? null} cluster={current?.cluster} local={current?.local} release={RELEASE} onBack={onBack} onCreate={onCreate} {...props} />);
  return { onBack, onCreate };
};
const section = (name: string) => screen.getByRole("region", { name });
const readText = (blob: Blob) =>
  new Promise<string>((resolve) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.readAsText(blob);
  });
const ready = () => screen.findByText("Asks for your passkey before 1 sensitive action");

beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => undefined) } });
});
afterEach(() => {
  cleanup();
  current = null;
});

describe("WorkspaceReview", () => {
  it("shows what the app is, what it can do, where it runs and what happens to the data", async () => {
    connect(sealed);
    review();
    expect(screen.getByRole("heading", { level: 2, name: "Review before creating" })).toBeTruthy();
    await ready();
    for (const name of ["Where it runs", "Your data", "What it can do", "Technical details"]) expect(section(name)).toBeTruthy();
    expect(screen.getByText("Mero CRM")).toBeTruthy();
    expect(screen.getByText("Version 1.4.0")).toBeTruthy();
    expect(within(section("Where it runs")).getByText("Verified")).toBeTruthy();
    const tech = section("Technical details");
    expect(within(tech).getByText("did:key:z6Mk…2doK")).toBeTruthy();
    expect(within(tech).getByText("bundle 7f3a…c91e, from the signed release")).toBeTruthy();
  });

  it("says the code comes from the signed release and never that the node attested it", async () => {
    const { attestApp } = connect(sealed);
    review();
    await ready();
    expect(screen.queryByText(/attested|Checking that this node/i)).toBeNull();
    expect(attestApp).not.toHaveBeenCalled();
  });

  it("copies the publisher's signer", async () => {
    connect(sealed);
    review();
    await ready();
    fireEvent.click(within(section("Technical details")).getByRole("button", { name: "Copy publisher" }));
    await waitFor(() => expect(navigator.clipboard.writeText).toHaveBeenCalledWith(card().signer));
  });

  it("says when the signer could not be copied", async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => Promise.reject(new Error("denied"))) } });
    connect(sealed);
    review();
    await ready();
    fireEvent.click(within(section("Technical details")).getByRole("button", { name: "Copy publisher" }));
    expect(await within(section("Technical details")).findByText("Could not copy")).toBeTruthy();
  });

  it("goes back or creates, and cannot create twice", async () => {
    connect(sealed);
    const { onBack, onCreate } = review();
    await ready();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(onCreate).toHaveBeenCalledTimes(1);
    cleanup();
    review({ creating: true });
    expect((screen.getByRole("button", { name: "Creating…" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("renders Back and Create last, in the row the stylesheet sticks to the bottom of the scroll container", async () => {
    connect(sealed);
    const { container } = render(<WorkspaceReview client={current?.client ?? null} cluster={current?.cluster} release={RELEASE} onBack={vi.fn()} onCreate={vi.fn()} />);
    await ready();
    const row = container.querySelector(".mero-cw-review")?.lastElementChild as HTMLElement;
    expect(row.className).toBe("mero-cw-review__buttons");
    expect(within(row).getAllByRole("button").map((b) => b.textContent)).toEqual(["Back", "Create workspace"]);
  });

  it("moves focus to the heading so a keyboard user starts at the review", async () => {
    connect(sealed);
    review();
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Review before creating" }));
  });

  it("says what loads while the card loads", async () => {
    let finish: (c: PackageCapabilities) => void = () => undefined;
    connect(sealed, () => new Promise((resolve) => (finish = resolve)));
    review();
    expect(screen.getByRole("status").textContent).toContain("Loading what this app can do");
    expect(within(section("Your data")).getByText(/not known until the app's details load/)).toBeTruthy();
    await act(async () => finish(card()));
    await ready();
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("what it can do", () => {
  it("opens with the summary and lists the actions in groups behind See all", async () => {
    connect(sealed);
    review();
    await ready();
    const can = section("What it can do");
    expect(within(can).getByText("Runs 1 task on its own")).toBeTruthy();
    expect(within(can).queryByText(/No other app can use your data/)).toBeNull();
    // What anyone can see and the outside services are named once, under Your data.
    expect(within(can).queryByText("Shows 1 thing to anyone")).toBeNull();
    expect(within(can).queryByText("Contacts 1 outside service")).toBeNull();
    expect(within(section("Your data")).getByText("Anyone can see: Pipeline summary")).toBeTruthy();
    const tech = section("Technical details");
    expect(within(tech).getByText("See all 4 actions")).toBeTruthy();
    for (const label of ["Needs your passkey", "Visible to anyone", "Changes your data", "Reads your data", "Runs on its own", "Outside services", "Updates it broadcasts"]) {
      expect(within(tech).getByText(label)).toBeTruthy();
    }
  });

  it("says no other app can use the data, with a check icon, when nothing is public or shared", async () => {
    connect(sealed, async () => closed());
    review();
    await screen.findByText("Asks for your passkey before 1 sensitive action");
    const line = within(section("What it can do")).getByText("No other app can use your data").closest("li") as HTMLElement;
    expect(line.getAttribute("data-tone")).toBe("ok");
    expect(line.querySelector("path")?.getAttribute("d")).toBe(CHECK_PATH);
  });

  it("separates what anyone can see from what anyone can do, in Level 1 and as an exception", async () => {
    const withCall = card();
    withCall.abi!.methods.joinWaitlist = method({ public: true });
    connect(sealed, async () => withCall);
    review();
    await ready();
    expect(within(section("What it can do")).queryByText("Lets anyone use 1 action without signing in")).toBeNull();
    expect(within(section("Your data")).getByText("Anyone can change: Join waitlist")).toBeTruthy();
    expect(within(section("Your data")).getByText("Anyone can see: Pipeline summary")).toBeTruthy();
  });

  it("filters the groups with the chips", async () => {
    connect(sealed);
    review();
    await ready();
    const can = section("Technical details");
    const all = within(can).getByRole("button", { name: /^All/ });
    const publicChip = within(can).getByRole("button", { name: "Public (1)" });
    expect(all.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(publicChip);
    expect(publicChip.getAttribute("aria-pressed")).toBe("true");
    expect(within(can).queryByText("Needs your passkey")).toBeNull();
    expect(within(can).getByText("Visible to anyone")).toBeTruthy();
    fireEvent.click(all);
    expect(within(can).getByText("Needs your passkey")).toBeTruthy();
  });

  it("expands an action into its technical facts, the publisher's words kept apart", async () => {
    connect(sealed);
    review();
    await ready();
    const can = section("Technical details");
    const passkey = within(can).getByText("Needs your passkey").closest("details") as HTMLElement;
    const row = within(passkey).getByText("Delete workspace").closest("details") as HTMLDetailsElement;
    expect(row.open).toBe(false);
    expect(within(row).getByText("deleteWorkspace")).toBeTruthy();
    expect(within(row).getByText("Passkey prompt every time")).toBeTruthy();
    expect(within(row).getByText("workspaceId: string")).toBeTruthy();
    expect(row.querySelector("summary [data-publisher]")?.textContent).toBe("Removes the workspace for good.");
    const words = within(row.querySelector(".mero-cw-review__facts") as HTMLElement).getByText("Removes the workspace for good.");
    expect(words.closest("[data-publisher]")?.textContent).toContain("Publisher's description");
    expect(row.querySelector("dl")?.textContent).not.toContain("Removes the workspace");
  });

  it("shows only the first sentence of a description on the row, and the whole text as plain text when expanded", async () => {
    const doc = "Moves an issue to another column. `status` is exactly\n`Open` or `Done` (else `Invalid`).\n\nEmits <b>IssueMoved</b>.";
    const c = card();
    c.abi!.methods.moveIssue = method({ doc });
    connect(sealed, async () => c);
    review();
    await ready();
    const row = within(section("Technical details")).getAllByText("Move issue")[0]!.closest("details") as HTMLDetailsElement;
    const said = row.querySelector("summary [data-publisher]");
    expect(said?.textContent).toBe("Moves an issue to another column.");
    const full = row.querySelector(".mero-cw-review__facts [data-publisher]") as HTMLElement;
    expect(full.textContent).toContain("Publisher's description");
    expect(full.textContent).toContain("`status` is exactly `Open` or `Done` (else `Invalid`).".replace(/`/g, ""));
    expect(full.querySelectorAll("p")).toHaveLength(2);
    expect(full.querySelector("code")?.textContent).toBe("status");
    expect(full.querySelector("b")).toBeNull();
    expect(full.textContent).toContain("<b>IssueMoved</b>");
  });

  it("shows the raw signed facts and downloads the full ABI as JSON", async () => {
    connect(sealed);
    review();
    await ready();
    const raw = within(section("Technical details")).getByText("Signed facts").closest("details") as HTMLElement;
    for (const text of ["manifest-hash", BUNDLE, card().signer, "mpk-hash"]) expect(within(raw).getByText(text)).toBeTruthy();

    const blobs: Blob[] = [];
    const created = vi.fn((blob: Blob) => (blobs.push(blob), "blob:abi"));
    const revoked = vi.fn();
    Object.assign(URL, { createObjectURL: created, revokeObjectURL: revoked });
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.download).toBe("network.calimero.crm-1.4.0-abi.json");
      expect(this.href).toBe("blob:abi");
    });
    fireEvent.click(within(raw).getByRole("button", { name: "Download full ABI (JSON)" }));
    expect(click).toHaveBeenCalledTimes(1);
    expect(JSON.parse(await readText(blobs[0]!))).toEqual(card().abi);
    click.mockRestore();
  });

  it("says the app publishes no action list when the card has no ABI, and still lists its outside services", async () => {
    connect(sealed, async () => card({ abi: null }));
    review();
    const can = section("What it can do");
    expect(await within(can).findByText("This app publishes no action list.")).toBeTruthy();
    expect(within(section("Your data")).getByText("Leaves the sealed hardware: api.stripe.com")).toBeTruthy();
    expect(within(can).queryByText(/No other app can use your data/)).toBeNull();
    expect(within(section("Technical details")).queryByRole("button", { name: "Download full ABI (JSON)" })).toBeNull();
    expect(within(section("Your data")).getByText(/publishes no action list/)).toBeTruthy();
  });
});

describe("where it runs", () => {
  it("names the cluster once and the release, and links the public verifier", async () => {
    connect(sealed);
    review();
    await ready();
    expect(within(section("Where it runs")).getByText("Calimero Cloud eu-shared-2")).toBeTruthy();
    const where = section("Technical details");
    expect(within(where).queryByText("Calimero Cloud eu-shared-2")).toBeNull();
    expect(within(where).getByText("mero-tee v2.3.148")).toBeTruthy();
    expect(within(where).getByRole("link", { name: /Verify/ }).getAttribute("href")).toBe(verifyNodeUrl(NODE, "2.3.148"));
  });

  it("breaks a long node host only after its dots, keeping the text whole", async () => {
    const host = "eu-shared-1-n1.relay.cloud.calimero.network";
    const url = `https://${host}`;
    connect({ ...sealed, url });
    review();
    await ready();
    const dd = within(section("Technical details")).getByText("Node").nextElementSibling as HTMLElement;
    expect(dd.textContent).toBe(host);
    expect(dd.querySelectorAll("wbr")).toHaveLength(4);
    const first = dd.firstElementChild as HTMLElement;
    expect(first.textContent).toBe("eu-shared-1-n1");
    expect(first.className).toBe("mero-cw-review__nobreak");
  });

  it("leaves a label with spaces to wrap at them", async () => {
    connect(sealed, async () => card(), undefined, undefined, "Calimero Cloud eu-shared-2");
    review();
    await ready();
    const dd = within(section("Where it runs")).getByText("Calimero Cloud eu-shared-2") as HTMLElement;
    expect(dd.querySelector(".mero-cw-review__nobreak")).toBeNull();
    expect(dd.querySelector("wbr")).toBeNull();
  });

  it("shows a local target once, as its label already says how many nodes there are", async () => {
    connect(mock, async () => card(), undefined, undefined, "merod local (3 nodes)", true);
    review();
    await ready();
    const where = section("Where it runs");
    expect(within(where).getAllByText(/3 nodes/)).toHaveLength(1);
    expect(within(where).queryByText("Nodes")).toBeNull();
    expect(within(where).getByText("merod local (3 nodes)")).toBeTruthy();
  });

  it("offers no verifier link and names no release for a test server", async () => {
    connect(mock);
    review();
    await ready();
    expect(within(section("Where it runs")).getByText("Not sealed")).toBeTruthy();
    const where = section("Technical details");
    expect(within(where).queryByRole("link")).toBeNull();
    expect(within(where).queryByText(/mero-tee v/)).toBeNull();
    expect(within(section("Your data")).getByText("Test server: not sealed")).toBeTruthy();
  });
});

describe("your data", () => {
  it("makes the three claims for a sealed node, with how we know", async () => {
    connect(sealed);
    review();
    await ready();
    const data = section("Your data");
    for (const text of ["Only you control it.", "Only people invited to it can open it.", "Calimero can't read it."]) expect(within(data).getByText(text)).toBeTruthy();
    expect(data.textContent).not.toContain("you invite");
    expect(data.textContent).not.toMatch(/you own/i);
    const how = within(section("Technical details")).getByText("How we know").closest("div") as HTMLElement;
    expect(within(how).getByText("Runs Calimero's published, signed node software (release v2.3.148)")).toBeTruthy();
    expect(within(how).getByText("Running in an Intel TDX enclave (not debug)")).toBeTruthy();
  });

  it("keeps the two org claims for a test server, and says it is not sealed in grey in place of the third", async () => {
    connect(mock);
    review();
    await ready();
    const data = section("Your data");
    expect(within(data).getByText("Only you control it, as this test server enforces it")).toBeTruthy();
    expect(within(data).getByText("Only people invited to it can open it, as this test server enforces it")).toBeTruthy();
    expect(data.querySelector(".mero-cw-shield__tick")).toBeNull();
    expect(within(data).queryByText("Calimero can't read it.")).toBeNull();
    expect(screen.queryByText("How we know")).toBeNull();
    expect(within(data).getByText("Test server: not sealed").closest("[data-kind]")?.getAttribute("data-kind")).toBe("unsealed");
  });

  it("lists an amber line for each public view and each outside host, for a test server as it lists them", async () => {
    connect(sealed);
    review();
    await ready();
    const data = section("Your data");
    expect(within(data).getByText("Anyone can see: Pipeline summary")).toBeTruthy();
    expect(within(data).getByText("Leaves the sealed hardware: api.stripe.com")).toBeTruthy();
    cleanup();
    current = null;
    connect(mock);
    review();
    await ready();
    const test = section("Your data");
    expect(within(test).getByText("Anyone can see: Pipeline summary")).toBeTruthy();
    expect(within(test).getByText("Contacts: api.stripe.com")).toBeTruthy();
    expect(within(test).getByText(/as this test server lists the app/i)).toBeTruthy();
    expect(within(test).queryByText(/sealed hardware/)).toBeNull();
  });
});

describe("a multi-service release", () => {
  it("asks for the card of its service", async () => {
    const { packageCapabilities } = connect(sealed);
    (packageCapabilities as ReturnType<typeof vi.fn>).mockImplementation(async (_pkg: string, _version: string, options?: { service?: string }) => {
      if (!options?.service) throw Object.assign(new Error("NO_SUCH_SERVICE"), { status: 400 });
      return card();
    });
    render(<WorkspaceReview client={current?.client ?? null} release={{ ...RELEASE, service: "web" }} onBack={vi.fn()} onCreate={vi.fn()} />);
    await ready();
    expect(packageCapabilities).toHaveBeenCalledWith(RELEASE.package, RELEASE.version, { service: "web" });
  });
});

describe("a card that cannot load", () => {
  it("says so plainly, still allows creating, and loads again on retry", async () => {
    const failure = Object.assign(new Error("404 ERROR: "), { status: 404 });
    const { packageCapabilities } = connect(sealed, async () => {
      throw failure;
    });
    const { onCreate } = review();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("This node cannot show app reviews yet.");
    expect(alert.textContent).not.toMatch(/404|ERROR/);
    expect(alert.textContent).toContain("You can still create the workspace");
    expect(within(section("What it can do")).getByText("Not available until the review loads.")).toBeTruthy();
    expect(within(section("Your data")).getByText(/not known until the app's details load/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    expect(onCreate).toHaveBeenCalledTimes(1);

    packageCapabilities.mockResolvedValueOnce(card());
    fireEvent.click(within(alert).getByRole("button", { name: "Try again" }));
    await ready();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(packageCapabilities).toHaveBeenCalledTimes(2);
  });

  it("does not offer to create when there is no node to create on", async () => {
    review();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Not connected to a node.");
    expect(alert.textContent).not.toContain("You can still create");
  });

  it.each([
    [401, "You don't have access to this app's details."],
    [403, "You don't have access to this app's details."],
    [500, "The review of this app could not load."],
    [undefined, "The review of this app could not load."],
  ])("says a %s plainly, never the raw error", async (status, said) => {
    connect(sealed, async () => {
      throw Object.assign(new Error(`${status} FORBIDDEN: nope`), status ? { status } : {});
    });
    review();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(said);
    expect(alert.textContent).not.toMatch(/FORBIDDEN|nope/);
  });
});

describe("a node that is not trusted", () => {
  it("labels the card as the test server's own listing, never the signed release", async () => {
    connect(mock, async () => closed());
    review();
    await screen.findByText("bundle 7f3a…c91e, from the release as this test server lists it");
    expect(screen.queryByText(/from the signed release/)).toBeNull();
    const can = section("What it can do");
    const tech = section("Technical details");
    expect(within(tech).getByText("Facts as this test server lists them")).toBeTruthy();
    const noApps = within(can).getByText("No other app can use your data, as this test server lists it").closest("li") as HTMLElement;
    expect(noApps.getAttribute("data-tone")).toBeNull();
    expect(noApps.querySelector("path")?.getAttribute("d")).toBe(CHECK_PATH);
    expect(within(can).queryByText("No other app can use your data")).toBeNull();
    expect(within(tech).queryByText("Signed facts")).toBeNull();
    fireEvent.click(within(tech).getByText(/^See all/));
    expect(within(tech).getByText(/come from the release as this test server lists it/)).toBeTruthy();
  });

  it("asks for the card only after the node's check verified it", async () => {
    let answer: (state: NodeVerificationState) => void = () => undefined;
    const { packageCapabilities } = connect({ status: "checking", url: NODE }, async () => card(), undefined, () => new Promise((resolve) => (answer = resolve)));
    review();
    expect(screen.getByRole("status").textContent).toContain("Loading what this app can do");
    expect(packageCapabilities).not.toHaveBeenCalled();
    await act(async () => answer(sealed));
    await ready();
    expect(packageCapabilities).toHaveBeenCalledTimes(1);
  });

  it("never asks a node that refused, shows the shield's refusal, and cannot create", async () => {
    const refused: NodeVerificationState = { status: "refused", url: NODE, reason: "wrong enclave key", checkedAt: Date.now() };
    const { packageCapabilities } = connect(refused);
    const { onCreate } = review();
    expect(await within(section("Your data")).findByText("Node refused")).toBeTruthy();
    expect(within(section("Your data")).getByText("This app refused the node and sent nothing.")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(screen.queryByText(/You can still create/)).toBeNull();
    expect(within(section("What it can do")).getByText("Not available: this node did not pass its check.")).toBeTruthy();
    const create = screen.getByRole("button", { name: "Create workspace" }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
    expect(screen.getByText("Create is unavailable: this node did not pass its check.")).toBeTruthy();
    fireEvent.click(create);
    expect(onCreate).not.toHaveBeenCalled();
    expect(packageCapabilities).not.toHaveBeenCalled();
  });

  it("never asks a node that cannot be reached", async () => {
    const unreachable: NodeVerificationState = { status: "unreachable", url: NODE, reason: "network down", checkedAt: Date.now() };
    const down = connect(unreachable);
    review();
    expect(screen.getByRole("status").textContent).toContain("Loading what this app can do");
    expect((screen.getByRole("button", { name: "Create workspace" }) as HTMLButtonElement).disabled).toBe(false);
    expect(down.packageCapabilities).not.toHaveBeenCalled();
  });
});
