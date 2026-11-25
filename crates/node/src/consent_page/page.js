// The node's consent page. Everything shown comes from the node
// (`GET /consent/{id}/model`); nothing here comes from the app. Text is set
// with textContent only, so no value from a manifest can become markup.
"use strict";
(function () {
  var id = decodeURIComponent(location.pathname.split("/")[2] || "");
  var base = "/consent/" + encodeURIComponent(id);
  var $ = function (sel) { return document.getElementById(sel); };
  // Withdrawing a release approval reads differently from approving.
  var withdrawing = false;
  // Checks that call for care: listed under Details, and repeated up top.
  var cautions = [];
  var CHALLENGE_FRESH_MS = 4 * 60 * 1000; // the node keeps a passkey challenge 5 minutes
  var INSTANT_MS = 1000; // a refusal sooner than this came back with no prompt shown
  // The approve challenge (the node's 412), asked before the click: browsers show
  // a passkey prompt only close to the click that asked for it.
  var challenge = null, asking = null, refresh = 0;
  // A prompt the browser refused without showing it: one more click asks again.
  var AGAIN = {};

  function el(tag, text, cls) {
    var e = document.createElement(tag);
    if (text !== undefined && text !== null) e.textContent = String(text);
    if (cls) e.className = cls;
    return e;
  }
  function add(parent, child) { parent.appendChild(child); return child; }
  function has(a) { return !!(a && a.length); }
  function hostOf(u) { try { return new URL(u).host; } catch (e) { return String(u || ""); } }
  // A long value on one line, cut with an ellipsis; the full value is its
  // title and stays selectable.
  function value(v, wrap) {
    var s = typeof v === "string" ? v : JSON.stringify(v);
    var e = el("span", s, wrap ? "v wrap" : "v");
    e.title = s;
    return e;
  }
  function link(url) {
    var a = el("a", url, "v"); a.href = url; a.rel = "noreferrer noopener"; a.title = url;
    return a;
  }
  function facts(rows) {
    var dl = el("dl");
    rows.forEach(function (r) {
      if (r[1] === undefined || r[1] === null || r[1] === "") return;
      var row = add(dl, el("div"));
      row.appendChild(el("dt", r[0]));
      var dd = add(row, el("dd"));
      dd.appendChild(r[1] instanceof Node ? r[1] : value(r[1], r[2]));
    });
    return dl;
  }
  // One part of a diff; nothing at all when it has nothing in it.
  function diff(parent, title, d) {
    if (d === null) {
      parent.appendChild(el("h3", title));
      parent.appendChild(el("p", "Unknown: a side of this upgrade embeds no ABI.", "muted"));
      return;
    }
    if (!d) return;
    var rows = [];
    (d.added || []).forEach(function (x) { rows.push([typeof x === "string" ? x : describe(x), "added"]); });
    (d.removed || []).forEach(function (x) { rows.push([typeof x === "string" ? x : describe(x), "removed"]); });
    (d.changed || []).forEach(function (x) { rows.push([describe(x), "changed"]); });
    if (!rows.length) return;
    parent.appendChild(el("h3", title));
    var ul = add(parent, el("ul", null, "diff"));
    rows.forEach(function (r) { var li = add(ul, el("li", r[0], r[1])); li.title = r[0]; });
  }
  function plainList(parent, title, items) {
    if (!has(items)) return;
    parent.appendChild(el("h3", title));
    var ul = add(parent, el("ul", null, "diff"));
    items.forEach(function (i) { var li = add(ul, el("li", i)); li.title = i; });
  }
  function describe(m) {
    if (m.everySecs) return m.name + " (every " + m.everySecs + " s)";
    if (m.fromSecs) return m.name + ": every " + m.fromSecs + " s → every " + m.toSecs + " s";
    if (m.from && m.to) return m.name + ": " + flags(m.from) + " → " + flags(m.to);
    return m.name + " (" + flags(m) + ")";
  }
  function flags(f) {
    var kinds = { view: "read-only view", init: "runs once at creation", timer: "runs on a timer", action: "runs in the background, may send data out" };
    var out = [kinds[f.kind] || "changes data"];
    if (f.confirm) out.push("needs a passkey confirmation");
    if (f.public) out.push("anyone, without signing in" + (f.rate ? " (" + f.rate + ")" : ""));
    if (has(f.allowApps)) out.push("callable by " + (f.allowApps.indexOf("*") >= 0 ? "other apps" : "the apps " + f.allowApps.join(", ")));
    if (f.allowSystem && f.kind !== "timer") out.push("called by the node itself");
    if (f.allowAction) out.push("called only by the app's background actions");
    if (f.params) out.push("takes (" + f.params.map(function (p) { return p[0]; }).join(", ") + ")");
    return out.join(", ");
  }
  function whom(scope) {
    if (!scope) return "";
    if (scope.kind === "org") return "the organization " + scope.name + " (" + scope.org + ")";
    if (scope.kind === "app") return "the app " + scope.name + " (" + scope.app + ")" + (scope.org ? ", organization " + scope.org : "");
    return "your own apps (account " + scope.account + ")";
  }
  // "Acme Inc." ends its own sentence.
  function end(s) { s = String(s); return /[.!?]$/.test(s) ? s : s + "."; }
  function shortKey(k) {
    k = String(k || "");
    return k.length > 24 ? k.slice(0, 14) + "…" + k.slice(-6) : k;
  }

  var EYEBROWS = {
    release: "Server version approval", upgrade: "Update request", upgradeUnit: "Update request",
    outbound: "Outbound access", createOrg: "New organization", createInvite: "Invite",
    acceptInvite: "Invitation", install: "New app", updatePolicy: "Update policy", updates: "Updates",
    setEgress: "Data leaving the cluster",
  };
  // The updates page (`updates`): approving unlocks its list instead of leaving.
  var hub = false;

  function render(m) {
    var host = location.host;
    document.querySelectorAll(".host").forEach(function (h) { h.textContent = host; });
    withdrawing = m.kind === "release" && m.release.change === "withdraw";
    if (withdrawing) $("approve-label").textContent = "Withdraw approval";
    $("eyebrow").textContent = withdrawing ? "Withdraw approval" : EYEBROWS[m.kind] || "Approval request";
    renderWho(m);
    $("headline").textContent = m.title || m.headline;
    renderVersion(m);
    renderMeans(m);
    renderChecks(m, host);
    renderFacts(m);
    renderExtra(m);
    var pill = el("span", cautions.length ? cautions.length + " to check" : "Checks passed", cautions.length ? "pill pill-warn" : "pill pill-ok");
    document.querySelector("#tech > summary").appendChild(pill);
    renderCode(m.code || "");
    renderTech(m);
    $("content").hidden = false;
    $("tech").hidden = false;
    $("meta").hidden = false;
    countdown(m.expiresAt, m.status === "pending");
    hub = m.kind === "updates";
    if (hub) {
      renderHub(m);
      return;
    }
    if (m.status === "pending") {
      $("actions").hidden = false;
    } else {
      $("status").textContent = "This request was " + m.status + " already.";
    }
  }

  // What the sheet's kind adds above the facts: the update's class and what changes,
  // the install sheet, a policy change. Every sentence is the node's.
  function section(parent, title) {
    var box = add(parent, el("section"));
    if (title) box.appendChild(el("h2", title));
    return box;
  }
  function bullets(parent, items) {
    var ul = add(parent, el("ul", null, "points"));
    (items || []).forEach(function (i) { if (i) ul.appendChild(el("li", i)); });
    return ul;
  }
  function badge(cls) {
    var name = cls === "green" ? "Green" : cls === "red" ? "Red" : "Unchecked";
    return el("span", name, "badge badge-" + (cls || "none"));
  }
  function labels(approve, decline, keepFirst, note) {
    if (approve) $("approve-label").textContent = approve;
    if (decline) $("decline").textContent = decline;
    if (keepFirst) {
      // The safe choice leads: keeping the version running now.
      $("decline").className = "primary";
      $("approve").className = "secondary";
    }
    if (note) { $("keep-note").textContent = note; $("keep-note").hidden = false; }
  }
  function renderExtra(m) {
    var x = $("extra");
    if ((m.kind === "upgrade" || m.kind === "upgradeUnit") && m.update) renderUpdate(x, m.update);
    if (m.kind === "install" && m.install) renderInstall(x, m.install);
    if (m.kind === "updatePolicy" && m.policy) renderPolicyChange(x, m.policy);
  }
  function renderUpdate(x, u) {
    var box = section(x);
    box.className = "class-" + (u.class || "none");
    var line = add(box, el("p", null, "class-line"));
    line.appendChild(badge(u.class));
    line.appendChild(el("span", u.summary));
    if (u.class === "red" && has(u.reasons)) bullets(box, u.reasons);
    var d = u.destinations || {};
    var where = el("span", d.words || "Unknown");
    if (has(d.added)) where.className = "plus";
    var rows = [["Destinations", where], ["Acts as you in other apps", Array.isArray(u.actsAs) ? u.actsAs.join(" ") : u.actsAs]];
    if (has(u.newMethods)) rows.push(["New methods", u.newMethods.join(", ")]);
    if (has(u.removedMethods)) rows.push(["Removed methods", u.removedMethods.join(", ")]);
    rows.push(["Stored data", u.stateVersion], ["Signed", u.signer]);
    if (has(u.members)) rows.push(["Apps it created", u.members.join(" · ")]);
    var dl = facts(rows.map(function (r) { return [r[0], r[1], true]; }));
    dl.className = "update-facts";
    box.appendChild(dl);
    var pol = section(x, "Updates of this app");
    bullets(pol, (u.policy && u.policy.words) || []);
    if (u.approvalWords) pol.appendChild(el("p", u.approvalWords, "note-box"));
    if (u.hold) pol.appendChild(el("p", u.hold, "note-box"));
    labels(u.approveLabel, u.keepLabel, u.keepByDefault, u.class === "red" ? u.keepWords : null);
  }
  // The organization chosen on an install sheet that asks for one; posted with the approval.
  var chosenOrg = null;
  function renderInstall(x, i) {
    if (has(i.chooseOrg)) chooseOrg(x, i.chooseOrg);
    var made = section(x, "What gets created for you");
    var ul = add(made, el("ul", null, "creates"));
    (i.creates || []).forEach(function (c, n) {
      var li = add(ul, el("li"));
      li.appendChild(el("span", c.name + " " + c.version, "n"));
      if (!n && i.privateTo) li.appendChild(el("span", i.privateTo, "pill pill-plain"));
      li.appendChild(el("span", c.what, "s"));
    });
    var where = section(x, "Where your data can go");
    bullets(where, has(i.destinations) ? i.destinations : ["Nowhere outside Calimero."]);
    where.appendChild(el("p", i.where, "small muted"));
    if (has(i.credentials)) {
      var creds = section(x, "Credentials the node attaches");
      bullets(creds, i.credentials);
      creds.appendChild(el("p", "Calimero adds each to its request itself; the app never reads it.", "small muted"));
    }
    var up = section(x, "Updates");
    bullets(up, i.updates);
    if (i.payer) x.appendChild(el("p", i.payer + " pays for this app; you have no Calimero bill for it.", "note-box"));
    labels("Create with passkey", "Cancel", false, null);
  }
  function chooseOrg(x, orgs) {
    var box = section(x, "Create it in");
    var select = add(box, el("select", null, "choose-org"));
    select.id = "org";
    select.setAttribute("aria-label", "Organization");
    var none = add(select, el("option", "Choose an organization"));
    none.value = ""; none.disabled = true; none.selected = true;
    orgs.forEach(function (o) { add(select, el("option", o.name)).value = o.orgId; });
    var points = {};
    orgs.forEach(function (o) { points[o.orgId] = o.point; });
    box.appendChild(el("p", "You are in several organizations. The app and its data belong to the one you choose.", "small muted"));
    chosenOrg = "";
    $("approve").disabled = true;
    select.addEventListener("change", function () {
      chosenOrg = select.value;
      $("approve").disabled = !chosenOrg;
      if (chosenOrg) prepare();
      if ($("created") && points[chosenOrg]) $("created").textContent = points[chosenOrg];
    });
  }
  function renderPolicyChange(x, p) {
    bullets(section(x, "Now"), p.now && p.now.words);
    bullets(section(x, "Once you approve"), p.afterWords);
    labels("Change with passkey", "Cancel", false, null);
  }

  // ---------------------------------------------------------------- updates page
  function renderHub(m) {
    var x = $("extra");
    while (x.firstChild) x.removeChild(x.firstChild);
    $("tech").hidden = true;
    if (m.locked !== false) {
      bullets(section(x), m.points);
      labels("Show my updates", "Back to the app", false, null);
      $("actions").hidden = m.status !== "pending";
      if (m.status !== "pending") $("status").textContent = "This page is no longer open. Ask the app again.";
      return;
    }
    $("content").querySelector(".match").hidden = true;
    $("attention").hidden = true;
    $("actions").hidden = true;
    var u = m.updates || {};
    var pending = u.pending || [];
    var tabs = add(x, el("div", null, "tabs"));
    tabs.setAttribute("role", "tablist");
    var panes = [];
    function tab(name, fill) {
      var b = add(tabs, el("button", name, "secondary"));
      b.type = "button"; b.setAttribute("role", "tab");
      var pane = add(x, el("section"));
      fill(pane);
      panes.push([b, pane]);
      b.addEventListener("click", function () { show(b); });
    }
    function show(which) {
      panes.forEach(function (p) { p[0].setAttribute("aria-selected", String(p[0] === which)); p[1].hidden = p[0] !== which; });
    }
    tab("Pending (" + pending.length + ")", function (pane) {
      if (!pending.length) pane.appendChild(el("p", "Nothing waits for your approval.", "muted"));
      pending.forEach(function (r) { hubRow(pane, r); });
      if (u.truncated) pane.appendChild(el("p", "More apps than this page reads at once: approve some, then reload.", "small muted"));
    });
    tab("Policies", function (pane) { (u.policies || []).forEach(function (o) { policyBlock(pane, o); }); });
    show(panes[0][0]);
    if (/[?&]result=approved/.test(location.search)) $("status").textContent = "Done. The list shows what waits now.";
  }
  function ago(at) {
    var h = Math.max(0, Math.floor((Date.now() / 1000 - at) / 3600));
    return h >= 48 ? Math.floor(h / 24) + " days" : h + " h";
  }
  function hubRow(pane, r) {
    var row = add(pane, el("div", null, "hub-row"));
    var left = add(row, el("div"));
    var top = add(left, el("p", null, "class-line"));
    top.appendChild(badge(r.class));
    top.appendChild(el("span", r.name + " " + (r.from || "?") + " \u2192 " + (r.to || "?"), "n"));
    left.appendChild(el("p", r.orgName + " · " + (r.summary || ""), "s"));
    if (r.approvedBy) {
      left.appendChild(el("p", (r.approvedByYou ? "You approved it first" : "Approved first by " + r.approvedBy) +
        (r.waitingSince ? ", " + ago(r.waitingSince) + " ago" : "") + "; it needs a second owner or admin.", "s"));
    }
    if (r.to && !r.approvedByYou && r.mayApprove !== false) {
      var b = add(row, el("button", "Review", "secondary"));
      b.type = "button";
      b.addEventListener("click", function () {
        openChild({ kind: r.unit ? "upgradeUnit" : "upgrade", subject: { app: r.app, toVersion: r.to } }, b);
      });
    }
  }
  function policyBlock(pane, o) {
    var box = add(pane, el("div", null, "policy"));
    box.appendChild(el("p", o.name, "n"));
    bullets(box, o.policy && o.policy.words);
    policyForm(box, o.policy || {}, function (policy) { return { org: o.org, policy: policy }; });
    (o.apps || []).forEach(function (a) {
      var sub = add(box, el("div", null, "policy"));
      sub.appendChild(el("p", (a.name || a.app) + ": its own policy", "n"));
      bullets(sub, a.policy && a.policy.words);
      var clear = add(sub, el("button", "Follow the organization's policy", "secondary"));
      clear.type = "button";
      clear.addEventListener("click", function () { openChild({ kind: "updatePolicy", subject: { app: a.app, policy: null } }, clear); });
    });
  }
  function policyForm(box, p, subject) {
    var form = add(box, el("div", null, "policy-form"));
    var autoL = add(form, el("label", "Apply on their own: "));
    var auto = add(autoL, el("select"));
    [["never", "never"], ["noDestinations", "updates that keep data in Calimero"]].forEach(function (o) {
      var opt = add(auto, el("option", o[1])); opt.value = o[0];
      if (p.autoApply === o[0]) opt.selected = true;
    });
    var holdL = add(form, el("label", "after "));
    var hold = add(holdL, el("input"));
    hold.type = "number"; hold.min = "24"; hold.max = "168"; hold.value = String(p.holdHours || 24);
    holdL.appendChild(document.createTextNode(" h"));
    var secondL = add(form, el("label"));
    var second = add(secondL, el("input"));
    second.type = "checkbox"; second.checked = !!p.secondApprover;
    secondL.appendChild(document.createTextNode(" a second approver for red updates"));
    var save = add(form, el("button", "Change with passkey", "secondary"));
    save.type = "button";
    save.addEventListener("click", function () {
      openChild({ kind: "updatePolicy", subject: subject({
        autoApply: auto.value, holdHours: parseInt(hold.value, 10) || 24, secondApprover: second.checked,
      }) }, save);
    });
  }
  // A sheet for one update or policy change, on this node, returning here.
  function openChild(body, button) {
    button.disabled = true;
    $("status").textContent = "";
    post("/open", body).then(function (r) {
      if (r.status !== 200) throw new Error((r.body && r.body.message) || "refused: " + r.status);
      var u = new URL(r.body.url);
      if (u.origin !== location.origin) throw new Error("bad page address");
      location.assign(u.href);
    }).catch(function (e) { button.disabled = false; $("status").textContent = e.message; });
  }
  function load() {
    return fetch(base + "/model", { credentials: "omit" })
      .then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error(j.message || r.status); return j; }); });
  }
  function renderVersion(m) {
    var u = m.kind === "upgrade" && m.upgrade;
    if (!u || !u.from || !u.to || !u.from.version || !u.to.version) return;
    $("ver-from").textContent = u.from.version;
    $("ver-to").textContent = u.to.version;
    $("ver").hidden = false;
  }
  // The code in two halves, the way people read it back.
  function renderCode(code) {
    var box = $("code");
    code.split("-").forEach(function (part, i) {
      if (i) box.appendChild(el("span", "-", "dash"));
      box.appendChild(el("span", part));
    });
  }
  // Time left, not a clock time: a person at the page does no clock math.
  function countdown(expiresAt, pending) {
    var total = Math.max(1, expiresAt - Math.floor(Date.now() / 1000));
    var ring = $("ring"), out = $("expires"), timer = null;
    var at = new Date(expiresAt * 1000).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    out.title = "Expires at " + at;
    function tick() {
      var left = expiresAt - Math.floor(Date.now() / 1000);
      if (left <= 0) {
        out.textContent = "Expired";
        ring.setAttribute("stroke-dashoffset", "50.27");
        if (timer) clearInterval(timer);
        if (pending && !$("approve").disabled) {
          $("actions").hidden = true;
          $("status").textContent = "This request expired. Ask again from the app.";
        }
        return;
      }
      var mins = Math.floor(left / 60), secs = left % 60;
      out.textContent = mins >= 60 ? "Expires at " + at : "Expires in " + mins + ":" + (secs < 10 ? "0" : "") + secs;
      ring.setAttribute("stroke-dashoffset", String((1 - Math.min(1, left / total)) * 50.27));
    }
    tick();
    timer = setInterval(tick, 1000);
  }
  function renderWho(m) {
    var by = m.requestedBy || {}, name, publisher;
    if (m.app) { name = m.app.name; publisher = m.publisher && m.publisher.name; }
    else if (by.name) { name = by.name; publisher = by.publisher; }
    else { name = hostOf(by.returnOrigin) || "A page of your cluster"; }
    $("who-name").textContent = name;
    $("who-by").textContent = publisher ? "by " + publisher : "";
    $("avatar").textContent = (name || "?").trim().charAt(0).toUpperCase();
    var acct = "Signed in as " + (m.approver && m.approver.account);
    if (m.scope && m.scope.kind === "org") acct += " · for " + m.scope.name;
    $("who-account").textContent = acct;
    $("who").hidden = false;
  }
  function renderMeans(m) {
    var points = m.points || [m.before, m.after];
    var ul = $("points");
    points.forEach(function (p) { if (p) ul.appendChild(el("li", p)); });
  }
  // The publisher's signature as one line of the main view, when it holds up.
  var signed = null;
  function check(ul, strong, rest, sub, note) {
    var li = add(ul, el("li", null, note ? "note" : ""));
    li.appendChild(el("strong", strong));
    if (rest) li.appendChild(document.createTextNode(rest));
    if (sub) li.appendChild(el("span", sub, "sub"));
    if (note) cautions.push([strong + (rest || ""), sub]);
    return li;
  }
  var ICONS = {
    warn: ["M10.3 4.2L2.6 18a2 2 0 0 0 1.7 3h15.4a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0z", "M12 9.5v4.5M12 17.3v.2"],
    shield: ["M12 3l7 3v5c0 4.5-3 8.3-7 10-4-1.7-7-5.5-7-10V6z", "M9 12l2 2 4-4"],
    key: ["M11 12l8.5-8.5M16.5 6.5l2.5 2.5M14 9l2 2", "M12 15a4 4 0 1 1-8 0 4 4 0 0 1 8 0z"],
    info: ["M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z", "M12 11v5.5M12 7.8v.2"],
  };
  function icon(name) {
    var ns = "http://www.w3.org/2000/svg", svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 24 24"); svg.setAttribute("class", "ic"); svg.setAttribute("aria-hidden", "true");
    ICONS[name].forEach(function (d) { var p = document.createElementNS(ns, "path"); p.setAttribute("d", d); svg.appendChild(p); });
    return svg;
  }
  function fact(kind, name, title, sub, items) {
    var li = add($("attention"), el("li", null, "fact " + kind));
    add(li, el("span", null, "fact-ic")).appendChild(icon(name));
    var body = add(li, el("div"));
    var t = add(body, el("p", title, "fact-t"));
    if (sub) body.appendChild(el("p", sub, "fact-s"));
    if (has(items)) {
      var box = add(body, el("p", null, "fact-items"));
      items.forEach(function (i) { var c = add(box, el("code", i)); c.title = i; });
    }
    return t;
  }
  // The main view: what needs care first (in colour), then what holds up,
  // then what happens. Only risk gets colour, so a warning stands out.
  function renderFacts(m) {
    var attn = m.attention || [];
    attn.forEach(function (a) { fact("warn", "warn", a.text, null, a.items); });
    cautions.forEach(function (c) { fact("warn", "warn", c[0], c[1]); });
    if (!attn.length && (m.kind === "upgrade" || m.kind === "upgradeUnit")) fact("ok", "shield", "No new permissions", "It reaches what the version you run now reaches.");
    if (signed) fact("ok", "key", signed[0], signed[1]);
    // Who chooses this app's updates: a deployment's name is not unique, so its line names the owner too.
    if (m.offeredBy) fact("", "info", m.offeredBy.title, m.offeredBy.detail);
    var points = (m.points || []).filter(Boolean);
    if (points.length) fact("", "info", points[0], points[1]).id = "created";
  }
  function renderChecks(m, host) {
    var ul = $("checks"), mockShown = false;
    check(ul, "Served by your node at " + host + ",", " not by the app.", "Your browser's address bar should show " + host + ".");
    check(ul, "Your passkey will ask to sign in to " + m.rpId + ".", "", "If it shows any other site, cancel.");
    var p = m.publisher;
    var upgrade = m.kind === "upgrade" || m.kind === "upgradeUnit";
    if (p && p.org && (upgrade || m.kind === "outbound")) {
      check(ul, end("Published by " + p.org.name), "", "Its signing key is bound to the organization " + p.org.name +
        " on your cluster, and your node verified the signature" + (p.keyChanged ? ", though the key differs from the version you run now." : "."), p.keyChanged);
      if (!p.keyChanged) signed = ["Published by " + p.org.name, "Its signing key is bound to " + p.org.name + " on your cluster."];
    } else if (p && upgrade) {
      if (p.keyChanged) {
        check(ul, "Published by " + (p.name || "an unnamed publisher") + ", signed with a new key.", "", "Your node verified the signature, but the key differs from the version you run now (" + shortKey(p.signer) + ").", true);
      } else {
        check(ul, end("Published and signed by " + (p.name || "the app's publisher")), "", "Your node verified the signature: the same key as the version you run now (" + shortKey(p.signer) + ").");
        signed = ["Signed by " + (p.name || "the app's publisher"), "Same key as the version you run now."];
      }
    } else if (p && m.kind === "outbound") {
      check(ul, end("Published and signed by " + (p.name || "the app's publisher")), "", "Your node verified the signature: the key this app has used since its first release (" + shortKey(p.signer) + ").");
      signed = ["Signed by " + (p.name || "the app's publisher"), "The key this app has used since its first release."];
    } else if (m.kind === "release" && !withdrawing) {
      if (m.release.measurements && m.release.measurements.mock) {
        check(ul, "Test cluster: no hardware checks.", "", "This cluster runs without hardware attestation, so there are no measurements to verify. Use it for testing only.", true);
        mockShown = true;
      } else {
        check(ul, "Built and published by Calimero's public release process.", "", "Your node verified its signature before showing you this page.");
        signed = ["Built by Calimero's public release process", "Your node verified its signature."];
      }
    }
    verifier(ul, m.verifier, mockShown);
  }
  // The independent verifier: a link, never a check run here. This page is
  // served by the node it would check, so only the verifier's own origin can
  // say the node is genuine; the person checks the link opens there.
  function verifier(ul, v, mockShown) {
    if (!v) return;
    if (v.mock) {
      if (!mockShown) check(ul, "Test cluster: nothing to verify independently.", "", "This cluster runs without hardware attestation, so there is no attestation an independent verifier could check.", true);
      return;
    }
    var u;
    try { u = new URL(v.url); } catch (e) { return; }
    if (u.protocol !== "https:" && u.protocol !== "http:") return;
    var li = add(ul, el("li"));
    var strong = add(li, el("strong", "Check this server's hardware and software on "));
    var a = add(strong, el("a", v.host + " \u2197"));
    a.href = u.href; a.target = "_blank"; a.rel = "noopener noreferrer"; a.title = u.href;
    li.appendChild(el("span", "An independent verifier checks this node's hardware attestation and its release (mero-tee " + v.release +
      ") on its own site. Check that the page opens on " + v.host + ", not on this page's address.", "sub"));
  }

  function renderTech(m) {
    var t = $("tech-body");
    t.appendChild(el("h3", "This request"));
    var by = m.requestedBy || {};
    t.appendChild(facts([
      ["Request", m.requestId],
      ["Approving as", m.approver && m.approver.account],
      ["For", whom(m.scope), true],
      ["Asked by", by.package ? "the app " + by.package : "a page on this cluster"],
      ["Returns you to", by.returnOrigin],
      ["Expires", new Date(m.expiresAt * 1000).toLocaleString()],
    ]));
    t.appendChild(el("h3", "In full"));
    [m.headline, m.before, m.after].concat(m.notes || []).forEach(function (s) { if (s) t.appendChild(el("p", s)); });
    if (m.kind === "release") techRelease(t, m.release);
    if (m.kind === "upgrade") techUpgrade(t, m.upgrade);
    if (m.kind === "delegate") techDelegate(t, m.delegate);
    // A unit: each app's own sheet, the instance first.
    if (m.kind === "upgradeUnit") {
      (m.apps || []).forEach(function (a) {
        t.appendChild(el("h3", a.headline));
        techUpgrade(t, a.upgrade);
      });
    }
    if (m.kind === "outbound") techOutbound(t, m.outbound);
  }
  function techDelegate(t, d) {
    t.appendChild(facts([
      ["Acts for you", d.source.name + " (" + d.source.app + ")"],
      ["In", d.target.name + " (" + d.target.app + ")"],
    ]));
    t.appendChild(el("h3", "Methods it may call as you"));
    var ul = add(t, el("ul"));
    d.methods.forEach(function (m) { ul.appendChild(el("li", m)); });
    if (d.alreadyGranted && d.alreadyGranted.length) {
      t.appendChild(el("p", "Already allowed: " + d.alreadyGranted.join(", ") + "."));
    }
  }
  function techRelease(t, r) {
    t.appendChild(el("h3", "The release"));
    if (r.change === "withdraw") {
      // Withdrawing lets nothing in: no measurements to check.
      t.appendChild(facts([
        ["Version", r.version],
        ["Approved now", r.currentlyApproved ? "yes" : "no"],
        ["Only one approved", r.lastApproved ? "yes: the node will refuse" : "no"],
        ["All approved now", (r.approved || []).join(", ")],
        ["Nodes on it", has(r.nodes) ? r.nodes.join(", ") : "none attested yet"],
        ["Profile", r.profile],
      ]));
      return;
    }
    t.appendChild(facts([
      ["Version", r.version],
      ["Already approved", r.alreadyApproved ? "yes" : "no"],
      ["Approved now", (r.approved || []).join(", ")],
      ["Nodes on it", has(r.nodes) ? r.nodes.join(", ") : "none attested yet"],
      ["Profile", r.profile],
      ["Measurements digest", r.measurements.policyDigest],
      ["Measurements", r.measurements.mock, true],
      ["Signed release", link(r.links.release)],
      ["Published measurements", link(r.links.measurements)],
      ["Deadline", r.deadline && (r.deadline.after + (r.deadline.passed ? " (passed)" : ""))],
    ]));
    if (r.verify) {
      t.appendChild(el("h3", "Verify it yourself"));
      t.appendChild(el("p", r.verify));
    }
    var profiles = r.measurements.profiles || {};
    Object.keys(profiles).forEach(function (p) {
      var reg = profiles[p];
      t.appendChild(el("h3", "Registers, profile " + p));
      t.appendChild(facts(["mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"].filter(function (k) { return has(reg[k]); })
        .map(function (k) { return [k.toUpperCase(), reg[k].join(" ")]; })));
    });
  }
  function techUpgrade(t, u) {
    t.appendChild(el("h3", "What changes"));
    var mig = u.migration || {};
    t.appendChild(facts([
      ["From", u.from.package + " " + u.from.version],
      ["To", u.to.package + " " + u.to.version],
      ["Publisher key", u.publisher.signer],
      ["Author", u.publisher.author],
      ["Stored data", "kept as it is: an update runs no migration" + (mig.fromStateVersion != null && mig.toStateVersion != null ? " (state version " + mig.fromStateVersion + " → " + mig.toStateVersion + ")" : ""), true],
      ["Frontend", u.frontend && (u.frontend.from || u.frontend.to) ? (u.frontend.from || "none") + " → " + (u.frontend.to || "none") : null],
    ]));
    var f = u.frontend || {};
    plainList(t, "New sign-in addresses", f.newLoginOrigins);
    plainList(t, "Sign-in addresses dropped", f.droppedLoginOrigins);
    diff(t, "Outbound addresses (approved separately)", u.outbound);
    plainList(t, "Secrets, new or bound elsewhere", u.secrets && u.secrets.words);
    diff(t, "Frontend aliases", f.aliases);
    diff(t, "Interfaces used", u.interfaces && u.interfaces.uses);
    diff(t, "Interfaces exported", u.interfaces && u.interfaces.exports);
    diff(t, "Methods", u.methods);
    diff(t, "Events", u.events);
    diff(t, "Timers", u.timers);
    t.appendChild(el("h3", "Exactly what you approve"));
    var b = u.buildInfo || {};
    t.appendChild(facts([
      ["Target manifest", u.to.manifestHash],
      ["Target package file", u.to.mpk],
      ["Target code", u.to.bundle],
      ["Current manifest", u.from.manifestHash],
      ["Build", b.to ? JSON.stringify(b.to) : null],
      ["Previous build", b.from ? JSON.stringify(b.from) : null],
    ]));
  }
  function techOutbound(t, o) {
    t.appendChild(el("h3", "Addresses"));
    t.appendChild(facts([
      ["Package", o.package],
      ["Publisher key", o.publisher],
      ["Release", o.release],
    ]));
    plainList(t, "Approving", o.requested);
    plainList(t, "New among them", o.new);
    plainList(t, "Already approved", o.alreadyApproved);
  }

  function b64urlToBytes(s) {
    s = s.replace(/-/g, "+").replace(/_/g, "/");
    while (s.length % 4) s += "=";
    var bin = atob(s), out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  function bytesToB64url(buf) {
    var bytes = new Uint8Array(buf), bin = "";
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function post(path, body) {
    return fetch(base + path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body || {}),
      credentials: "omit",
    }).then(function (r) { return r.json().then(function (j) { return { status: r.status, body: j }; }); });
  }
  function leave(url) {
    var u = new URL(url);
    if (u.protocol !== "https:" && u.protocol !== "http:") throw new Error("bad redirect");
    location.assign(u.href);
  }
  function busy(on, text) {
    $("approve").disabled = on || chosenOrg === ""; $("decline").disabled = on;
    $("status").textContent = text || "";
  }
  // Where the approval is: 1 asks the node, 2 waits for the passkey, 3
  // records it; 0 puts the buttons back.
  function step(n) {
    var ol = $("steps");
    ol.hidden = !n;
    $("actions").hidden = !!n;
    for (var i = 0; i < ol.children.length; i++) {
      ol.children[i].className = i + 1 < n ? "did" : i + 1 === n ? "now" : "";
    }
    if (n) busy(true, "");
  }
  function finished() {
    var sheet = document.querySelector(".sheet");
    sheet.classList.add("finished");
    ["who", "ver", "content", "steps", "actions", "meta"].forEach(function (k) { $(k).hidden = true; });
    $("done").hidden = false;
    $("headline").textContent = withdrawing ? "Withdrawn" : "Approved";
    $("status").textContent = "Returning you to the app…";
  }

  // Ask the node for the approve challenge now, and again before the node forgets it.
  function prepare() {
    challenge = null;
    clearTimeout(refresh);
    // The chosen organization goes into the action, so the passkey signs it.
    var org = chosenOrg;
    var p = post("/approve", org ? { org: org } : {}).then(function (r) {
      if (r.status !== 412) throw new Error((r.body && r.body.message) || "unexpected answer " + r.status);
      if (asking === p) {
        challenge = { d: r.body.details, at: Date.now(), org: org };
        refresh = setTimeout(prepare, CHALLENGE_FRESH_MS);
      }
      return r.body.details;
    });
    p.catch(function () { if (asking === p) asking = null; });
    return (asking = p);
  }
  // The passkey over `d`, then the approval. Called inside the click when `d` was ready.
  function ask(d) {
    step(2);
    var started = Date.now();
    return navigator.credentials.get({ publicKey: {
      challenge: b64urlToBytes(d.challenge),
      rpId: d.rpId,
      allowCredentials: (d.allowCredentials || []).map(function (c) { return { type: "public-key", id: b64urlToBytes(c) }; }),
      userVerification: "required",
      timeout: 120000,
    } }).then(function (cred) {
      step(3);
      challenge = asking = null;
      clearTimeout(refresh);
      return post("/approve", Object.assign({ confirm: { challengeId: d.challengeId, assertion: {
        credentialId: bytesToB64url(cred.rawId),
        authenticatorData: bytesToB64url(cred.response.authenticatorData),
        clientDataJson: bytesToB64url(cred.response.clientDataJSON),
        signature: bytesToB64url(cred.response.signature),
      } } }, chosenOrg ? { org: chosenOrg } : {})).then(function (r) {
        if (r.status === 200) return r;
        var code = r.body && r.body.error;
        if (code === "EXPIRED_CHALLENGE" || code === "UNKNOWN_CHALLENGE") {
          return prepare().then(function () { throw AGAIN; });
        }
        prepare();
        throw new Error((r.body && r.body.message) || "refused: " + r.status);
      });
    }, function (e) {
      throw e && e.name === "NotAllowedError" && Date.now() - started < INSTANT_MS ? AGAIN : e;
    });
  }
  function approve() {
    var c = challenge;
    var asked;
    if (c && c.org === chosenOrg && Date.now() - c.at < CHALLENGE_FRESH_MS) {
      asked = ask(c.d);
    } else {
      step(1);
      asked = (c || !asking || (challenge && challenge.org !== chosenOrg) ? prepare() : asking).then(ask);
    }
    asked.then(function (r) {
      if (hub) {
        // Unlocked: the page reads its list afresh.
        location.reload();
        return;
      }
      finished();
      leave(r.body.redirect);
    }).catch(function (e) {
      step(0);
      if (e === AGAIN) {
        $("approve-label").textContent = "Confirm with your passkey";
        busy(false, "Your browser wants one more tap before it shows the passkey prompt.");
        return;
      }
      busy(false, (withdrawing ? "Not withdrawn: " : "Not approved: ") + e.message);
    });
  }
  function decline() {
    busy(true, "Declining…");
    post("/decline").then(function (r) {
      if (r.status !== 200) throw new Error((r.body && r.body.message) || "refused: " + r.status);
      leave(r.body.redirect);
    }).catch(function (e) { busy(false, e.message); });
  }

  $("approve").addEventListener("click", approve);
  $("decline").addEventListener("click", decline);
  load()
    .then(render)
    // A choice of organization still pending: its challenge is asked for once one is chosen.
    .then(function () { if (!$("actions").hidden && chosenOrg !== "") prepare(); })
    .catch(function (e) {
      document.querySelectorAll(".host").forEach(function (h) { h.textContent = location.host; });
      $("headline").textContent = "Nothing to approve here";
      $("status").textContent = String(e.message || e);
    });
})();
