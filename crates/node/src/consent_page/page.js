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
    release: "Server version approval", upgrade: "Update request", outbound: "Outbound access",
    createOrg: "New organization", createInvite: "Invite", acceptInvite: "Invitation",
  };

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
    var pill = el("span", cautions.length ? cautions.length + " to check" : "Checks passed", cautions.length ? "pill pill-warn" : "pill pill-ok");
    document.querySelector("#tech > summary").appendChild(pill);
    renderCode(m.code || "");
    renderTech(m);
    $("content").hidden = false;
    $("tech").hidden = false;
    $("meta").hidden = false;
    countdown(m.expiresAt, m.status === "pending");
    if (m.status === "pending") {
      $("actions").hidden = false;
    } else {
      $("status").textContent = "This request was " + m.status + " already.";
    }
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
    body.appendChild(el("p", title, "fact-t"));
    if (sub) body.appendChild(el("p", sub, "fact-s"));
    if (has(items)) {
      var box = add(body, el("p", null, "fact-items"));
      items.forEach(function (i) { var c = add(box, el("code", i)); c.title = i; });
    }
  }
  // The main view: what needs care first (in colour), then what holds up,
  // then what happens. Only risk gets colour, so a warning stands out.
  function renderFacts(m) {
    var attn = m.attention || [];
    attn.forEach(function (a) { fact("warn", "warn", a.text, null, a.items); });
    cautions.forEach(function (c) { fact("warn", "warn", c[0], c[1]); });
    if (!attn.length && (m.kind === "upgrade" || m.kind === "upgradeUnit")) fact("ok", "shield", "No new permissions", "It reaches what the version you run now reaches.");
    if (signed) fact("ok", "key", signed[0], signed[1]);
    var points = (m.points || []).filter(Boolean);
    if (points.length) fact("", "info", points[0], points[1]);
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
    // A unit: each app's own sheet, the instance first.
    if (m.kind === "upgradeUnit") {
      (m.apps || []).forEach(function (a) {
        t.appendChild(el("h3", a.headline));
        techUpgrade(t, a.upgrade);
      });
    }
    if (m.kind === "outbound") techOutbound(t, m.outbound);
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
    $("approve").disabled = on; $("decline").disabled = on;
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

  function approve() {
    step(1);
    post("/approve").then(function (r) {
      if (r.status !== 412) throw new Error((r.body && r.body.message) || "unexpected answer " + r.status);
      var d = r.body.details;
      step(2);
      return navigator.credentials.get({ publicKey: {
        challenge: b64urlToBytes(d.challenge),
        rpId: d.rpId,
        allowCredentials: (d.allowCredentials || []).map(function (c) { return { type: "public-key", id: b64urlToBytes(c) }; }),
        userVerification: "required",
        timeout: 120000,
      } }).then(function (cred) {
        step(3);
        return post("/approve", { confirm: { challengeId: d.challengeId, assertion: {
          credentialId: bytesToB64url(cred.rawId),
          authenticatorData: bytesToB64url(cred.response.authenticatorData),
          clientDataJson: bytesToB64url(cred.response.clientDataJSON),
          signature: bytesToB64url(cred.response.signature),
        } } });
      });
    }).then(function (r) {
      if (r.status !== 200) throw new Error((r.body && r.body.message) || "refused: " + r.status);
      finished();
      leave(r.body.redirect);
    }).catch(function (e) { step(0); busy(false, (withdrawing ? "Not withdrawn: " : "Not approved: ") + e.message); });
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
  fetch(base + "/model", { credentials: "omit" })
    .then(function (r) { return r.json().then(function (j) { if (!r.ok) throw new Error(j.message || r.status); return j; }); })
    .then(render)
    .catch(function (e) {
      document.querySelectorAll(".host").forEach(function (h) { h.textContent = location.host; });
      $("headline").textContent = "Nothing to approve here";
      $("status").textContent = String(e.message || e);
    });
})();
