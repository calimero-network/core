// The node's consent page. Everything shown comes from the node
// (`GET /consent/{id}/model`); nothing here comes from the app. Text is set
// with textContent only, so no value from a manifest can become markup.
"use strict";
(function () {
  var id = decodeURIComponent(location.pathname.split("/")[2] || "");
  var base = "/consent/" + encodeURIComponent(id);
  var $ = function (sel) { return document.getElementById(sel); };

  function el(tag, text, cls) {
    var e = document.createElement(tag);
    if (text !== undefined && text !== null) e.textContent = String(text);
    if (cls) e.className = cls;
    return e;
  }
  function list(items, cls) {
    var ul = el("ul");
    (items || []).forEach(function (i) { ul.appendChild(el("li", typeof i === "string" ? i : JSON.stringify(i), cls)); });
    if (!items || !items.length) ul.appendChild(el("li", "none"));
    return ul;
  }
  function facts(rows) {
    var dl = el("dl");
    rows.forEach(function (r) {
      if (r[1] === undefined || r[1] === null || r[1] === "") return;
      dl.appendChild(el("dt", r[0]));
      var dd = el("dd");
      if (r[1] instanceof Node) dd.appendChild(r[1]); else dd.appendChild(el("code", r[1]));
      dl.appendChild(dd);
    });
    return dl;
  }
  function diff(title, d) {
    var box = el("div", null, "box");
    box.appendChild(el("h2", title));
    if (!d) { box.appendChild(el("p", "Unknown: a side of this upgrade embeds no ABI.")); return box; }
    box.appendChild(el("p", "Added"));
    box.appendChild(list((d.added || []).map(function (x) { return typeof x === "string" ? x : describe(x); }), "added"));
    box.appendChild(el("p", "Removed"));
    box.appendChild(list((d.removed || []).map(function (x) { return typeof x === "string" ? x : describe(x); }), "removed"));
    if (d.changed && d.changed.length) {
      box.appendChild(el("p", "Changed"));
      box.appendChild(list(d.changed.map(describe)));
    }
    return box;
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

  function render(m) {
    $("headline").textContent = m.headline;
    var c = $("content");
    c.textContent = "";
    c.appendChild(facts([
      ["Approving as", m.approver && m.approver.account],
      ["For", whom(m.scope)],
      ["Asked by", m.requestedBy && (m.requestedBy.package ? "the app " + m.requestedBy.package : "a page on this cluster")],
      ["Returns you to", m.requestedBy && m.requestedBy.returnOrigin],
      ["Expires", new Date(m.expiresAt * 1000).toLocaleString()],
    ]));
    c.appendChild(el("p", m.before, "box before"));
    c.appendChild(el("p", m.after, "box after"));
    (m.notes || []).forEach(function (n) { c.appendChild(el("p", n, "box warn")); });
    if (m.kind === "release") renderRelease(c, m.release);
    if (m.kind === "upgrade") renderUpgrade(c, m.upgrade);
    if (m.kind === "outbound") renderOutbound(c, m.outbound);
    c.hidden = false;
    if (m.status === "pending") {
      $("actions").hidden = false;
    } else {
      $("status").textContent = "This request was " + m.status + " already.";
    }
  }
  function renderRelease(c, r) {
    c.appendChild(el("h2", "The release"));
    var link = el("a", r.links.release); link.href = r.links.release; link.rel = "noreferrer noopener";
    var meas = el("a", r.links.measurements); meas.href = r.links.measurements; meas.rel = "noreferrer noopener";
    c.appendChild(facts([
      ["Version", r.version],
      ["Already approved", r.alreadyApproved ? "yes" : "no"],
      ["Approved now", (r.approved || []).join(", ")],
      ["Nodes on it", (r.nodes || []).length ? r.nodes.join(", ") : "none attested yet"],
      ["Profile", r.profile],
      ["Measurements digest", r.measurements.policyDigest],
      ["Measurements", r.measurements.mock],
      ["Signed release", link],
      ["Published measurements", meas],
      ["Verify it yourself", r.verify],
      ["Deadline", r.deadline],
    ]));
    if (r.measurements.profiles) {
      Object.keys(r.measurements.profiles).forEach(function (p) {
        var reg = r.measurements.profiles[p];
        c.appendChild(el("p", "Profile " + p));
        c.appendChild(facts([["MRTD", reg.mrtd.join(" ")], ["RTMR1", reg.rtmr1.join(" ")], ["RTMR2", reg.rtmr2.join(" ")], ["RTMR3", reg.rtmr3.join(" ")]]));
      });
    }
  }
  function renderUpgrade(c, u) {
    c.appendChild(el("h2", "What changes"));
    c.appendChild(facts([
      ["From", u.from.package + " " + u.from.version],
      ["To", u.to.package + " " + u.to.version],
      ["Publisher", u.publisher.signer + (u.publisher.author ? " (" + u.publisher.author + ")" : "")],
      ["Migration", u.migration.refused ? "refused: " + u.migration.refused : u.migration.runs ? "runs " + u.migration.export + " (state " + u.migration.fromStateVersion + " → " + u.migration.toStateVersion + ")" : "none: code only"],
      ["Frontend", (u.frontend.from || "none") + " → " + (u.frontend.to || "none")],
    ]));
    var origins = u.frontend.newLoginOrigins || [];
    if (origins.length) {
      var warn = el("div", null, "box warn");
      warn.appendChild(el("p", "New sign-in origins: pages at these addresses could act in this app as you."));
      warn.appendChild(list(origins));
      c.appendChild(warn);
    }
    c.appendChild(diff("Outbound addresses (approved separately)", u.outbound));
    c.appendChild(diff("Frontend aliases", u.frontend.aliases));
    c.appendChild(diff("Interfaces used", u.interfaces.uses));
    c.appendChild(diff("Interfaces exported", u.interfaces.exports));
    c.appendChild(diff("Public methods", u.methods));
    c.appendChild(diff("Events", u.events));
    c.appendChild(diff("Timers", u.timers));
    c.appendChild(el("h2", "Exactly what you approve"));
    c.appendChild(facts([
      ["Target manifest", u.to.manifestHash],
      ["Target package file", u.to.mpk],
      ["Target code", u.to.bundle],
      ["Current manifest", u.from.manifestHash],
      ["Build", JSON.stringify(u.buildInfo.to)],
      ["Previous build", JSON.stringify(u.buildInfo.from)],
    ]));
  }
  function renderOutbound(c, o) {
    c.appendChild(el("h2", "Addresses"));
    c.appendChild(facts([
      ["Package", o.package],
      ["Publisher", o.publisher],
      ["Release", o.release],
    ]));
    c.appendChild(el("p", "Approving"));
    c.appendChild(list(o.requested));
    c.appendChild(el("p", "New among them"));
    c.appendChild(list(o.new, "added"));
    c.appendChild(el("p", "Already approved"));
    c.appendChild(list(o.alreadyApproved));
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
        busy(true, "Recording your approval…");
        return post("/approve", { confirm: { challengeId: d.challengeId, assertion: {
          credentialId: bytesToB64url(cred.rawId),
          authenticatorData: bytesToB64url(cred.response.authenticatorData),
          clientDataJson: bytesToB64url(cred.response.clientDataJSON),
          signature: bytesToB64url(cred.response.signature),
        } } });
      });
    }).then(function (r) {
      if (r.status !== 200) throw new Error((r.body && r.body.message) || "refused: " + r.status);
      busy(true, "Approved. Returning you to the app…");
      leave(r.body.redirect);
    }).catch(function (e) { busy(false, "Not approved: " + e.message); });
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
    .catch(function (e) { $("headline").textContent = "Nothing to approve here"; $("status").textContent = String(e.message || e); });
})();
