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
    var out = [f.kind === "view" ? "read-only view" : "changes data"];
    if (f.xcall) out.push("callable by other apps");
    if (f.handler) out.push("event handler");
    if (f.destructive) out.push("deletes data");
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

  function render(m) {
    var host = location.host;
    document.querySelectorAll(".host").forEach(function (h) { h.textContent = host; });
    withdrawing = m.kind === "release" && m.release.change === "withdraw";
    if (withdrawing) $("approve").textContent = "Withdraw approval";
    renderWho(m);
    $("headline").textContent = m.title || m.headline;
    renderMeans(m);
    renderChecks(m, host);
    $("code").textContent = m.code || "";
    renderTech(m);
    $("content").hidden = false;
    $("tech").hidden = false;
    $("expires").textContent = "This request expires at " +
      new Date(m.expiresAt * 1000).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) + ".";
    if (m.status === "pending") {
      $("actions").hidden = false;
    } else {
      $("status").textContent = "This request was " + m.status + " already.";
    }
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
    var box = $("attention"), attn = m.attention || [];
    if (attn.length) {
      var w = add(box, el("div", null, "attn"));
      w.appendChild(el("h3", "Needs your attention"));
      var list = add(w, el("ul"));
      attn.forEach(function (a) {
        var li = add(list, el("li", a.text));
        if (has(a.items)) {
          var sub = add(li, el("ul"));
          a.items.forEach(function (i) { sub.appendChild(el("li", i)); });
        }
      });
    } else if (m.kind === "upgrade") {
      box.appendChild(el("p", "No new permissions.", "calm"));
    }
  }
  function check(ul, strong, rest, sub, note) {
    var li = add(ul, el("li", null, note ? "note" : ""));
    li.appendChild(el("strong", strong));
    if (rest) li.appendChild(document.createTextNode(rest));
    if (sub) li.appendChild(el("span", sub, "sub"));
  }
  function renderChecks(m, host) {
    var ul = $("checks");
    check(ul, "Served by your node at " + host + ",", " not by the app.", "Your browser's address bar should show " + host + ".");
    check(ul, "Your passkey will ask to sign in to " + m.rpId + ".", "", "If it shows any other site, cancel.");
    var p = m.publisher;
    if (p && p.org && (m.kind === "upgrade" || m.kind === "outbound")) {
      check(ul, end("Published by " + p.org.name), "", "Its signing key is bound to the organization " + p.org.name +
        " on your cluster, and your node verified the signature" + (p.keyChanged ? ", though the key differs from the version you run now." : "."), p.keyChanged);
    } else if (p && m.kind === "upgrade") {
      if (p.keyChanged) {
        check(ul, "Published by " + (p.name || "an unnamed publisher") + ", signed with a new key.", "", "Your node verified the signature, but the key differs from the version you run now (" + shortKey(p.signer) + ").", true);
      } else {
        check(ul, end("Published and signed by " + (p.name || "the app's publisher")), "", "Your node verified the signature: the same key as the version you run now (" + shortKey(p.signer) + ").");
      }
    } else if (p && m.kind === "outbound") {
      check(ul, end("Published and signed by " + (p.name || "the app's publisher")), "", "Your node verified the signature: the key this app has used since its first release (" + shortKey(p.signer) + ").");
    } else if (m.kind === "release" && !withdrawing) {
      if (m.release.measurements && m.release.measurements.mock) {
        check(ul, "Test cluster: no hardware checks.", "", "This cluster runs without hardware attestation, so there are no measurements to verify. Use it for testing only.", true);
      } else {
        check(ul, "Built and published by Calimero's public release process.", "", "Your node verified its signature before showing you this page.");
      }
    }
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
      ["Migration", mig.refused ? "refused: " + mig.refused : mig.runs ? "runs " + mig.export + " (state " + mig.fromStateVersion + " → " + mig.toStateVersion + ")" : "none: code only", true],
      ["Frontend", u.frontend && (u.frontend.from || u.frontend.to) ? (u.frontend.from || "none") + " → " + (u.frontend.to || "none") : null],
    ]));
    var f = u.frontend || {};
    plainList(t, "New sign-in addresses", f.newLoginOrigins);
    plainList(t, "Sign-in addresses dropped", f.droppedLoginOrigins);
    diff(t, "Outbound addresses (approved separately)", u.outbound);
    diff(t, "Frontend aliases", f.aliases);
    diff(t, "Interfaces used", u.interfaces && u.interfaces.uses);
    diff(t, "Interfaces exported", u.interfaces && u.interfaces.exports);
    diff(t, "Public methods", u.methods);
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

  function approve() {
    busy(true, "Asking the node for a challenge…");
    post("/approve").then(function (r) {
      if (r.status !== 412) throw new Error((r.body && r.body.message) || "unexpected answer " + r.status);
      var d = r.body.details;
      busy(true, "Confirm with your passkey…");
      return navigator.credentials.get({ publicKey: {
        challenge: b64urlToBytes(d.challenge),
        rpId: d.rpId,
        allowCredentials: (d.allowCredentials || []).map(function (c) { return { type: "public-key", id: b64urlToBytes(c) }; }),
        userVerification: "required",
        timeout: 120000,
      } }).then(function (cred) {
        busy(true, withdrawing ? "Withdrawing the approval…" : "Recording your approval…");
        return post("/approve", { confirm: { challengeId: d.challengeId, assertion: {
          credentialId: bytesToB64url(cred.rawId),
          authenticatorData: bytesToB64url(cred.response.authenticatorData),
          clientDataJson: bytesToB64url(cred.response.clientDataJSON),
          signature: bytesToB64url(cred.response.signature),
        } } });
      });
    }).then(function (r) {
      if (r.status !== 200) throw new Error((r.body && r.body.message) || "refused: " + r.status);
      busy(true, (withdrawing ? "Withdrawn." : "Approved.") + " Returning you to the app…");
      leave(r.body.redirect);
    }).catch(function (e) { busy(false, (withdrawing ? "Not withdrawn: " : "Not approved: ") + e.message); });
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
