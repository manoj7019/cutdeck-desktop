(function (root, factory) {
  var api = factory();
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof document !== "undefined") api.boot();
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  var REPO = "manoj7019/manojhustler";
  

  function esc(value) {
    return String(value == null ? "" : value).replace(/[&<>"']/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch];
    });
  }
  function each(sel, fn) { Array.prototype.forEach.call(document.querySelectorAll(sel), fn); }
  function filled(v) { return !!v && String(v).trim() !== "" && String(v).indexOf("REPLACE_ME") === -1; }

  function load(path) {
    var base = document.body.getAttribute("data-root") || ".";
    return fetch(base + "/" + path).then(function (res) {
      if (!res.ok) throw new Error(path);
      return res.json();
    });
  }

  // ---------- Downloads ----------
  // Newest release whose tag starts with prefix and has assetName (list is newest first).
  // Each app has its own tag prefix (Cutdeck "v", Bob Player "bobplayer-v"), so one
  // repo can hold releases for several apps.
  function pickRelease(releases, prefix, assetName) {
    if (!prefix || !assetName || !Array.isArray(releases)) return "";
    for (var i = 0; i < releases.length; i++) {
      var rel = releases[i] || {};
      if (String(rel.tag_name || "").indexOf(prefix) !== 0) continue;
      var assets = rel.assets || [];
      for (var j = 0; j < assets.length; j++) {
        var a = assets[j] || {};
        if (a.name === assetName && a.browser_download_url) return a.browser_download_url;
      }
    }
    return "";
  }

  function closestAttr(node, name) {
    for (var cur = node; cur && cur.getAttribute; cur = cur.parentElement) {
      var v = cur.getAttribute(name);
      if (v) return v;
    }
    return "";
  }

  function bindDownloads() {
    var nodes = document.querySelectorAll("[data-release-asset]");
    if (!nodes.length) return;
    var byRepo = {};
    Array.prototype.forEach.call(nodes, function (node) {
      var repo = closestAttr(node, "data-release-repo") || REPO;
      (byRepo[repo] || (byRepo[repo] = [])).push(node);
    });
    Object.keys(byRepo).forEach(function (repo) {
      fetch("https://api.github.com/repos/" + repo + "/releases?per_page=100", {
        headers: { Accept: "application/vnd.github+json" }
      })
        .then(function (res) { if (!res.ok) throw new Error("releases"); return res.json(); })
        .then(function (releases) {
          byRepo[repo].forEach(function (node) {
            var url = pickRelease(releases, closestAttr(node, "data-release-prefix"), node.getAttribute("data-release-asset"));
            if (url) node.href = url;
          });
        })
        .catch(function () {});
    });
  }

  // Put the visitor's own system first and make it the main button.
  function detectOS() {
    var ua = navigator.userAgent || "";
    var p = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    if (/Android|iPhone|iPad/i.test(ua)) return "";
    if (/Win/i.test(p) || /Windows/i.test(ua)) return "windows";
    if (/Mac/i.test(p) || /Mac OS X/i.test(ua)) return "mac";
    if (/Linux|X11/i.test(p + ua)) return "linux";
    return "";
  }
  function highlightOS() {
    var box = document.querySelector("[data-downloads]");
    if (!box) return;
    var os = detectOS();
    if (!os) return;
    var match = box.querySelector('[data-os="' + os + '"]');
    if (!match) return;
    each("[data-downloads] .btn", function (b) {
      b.classList.remove("primary");
      if (b.hasAttribute("data-short")) b.innerHTML = b.getAttribute("data-short");
    });
    match.classList.add("primary");
    if (match.hasAttribute("data-long")) match.innerHTML = match.getAttribute("data-long");
    box.insertBefore(match, box.firstChild);
  }

  // ---------- Home ----------
  function renderHome() {
    var list = document.querySelector("[data-app-list]");
    if (!list) return;
    var base = document.body.getAttribute("data-root") || ".";
    load("data/apps.json").then(function (data) {
      var apps = data.apps || [];
      if (!apps.length) return;
      list.innerHTML = apps.map(function (app) {
        var os = (app.platforms || []).map(function (n) { return "<span>" + esc(n) + "</span>"; }).join("");
        return (
          '<a class="app" href="' + esc(app.href) + '" style="--app:' + esc(app.color || "") + '">' +
            '<img src="' + esc(base + "/" + String(app.icon || "").replace(/^\//, "")) + '" alt="" width="96" height="96">' +
            "<div><h2>" + esc(app.name) + "</h2><p>" + esc(app.description) + '</p><div class="os">' + os + "</div></div>" +
            '<span class="go">Get ' + esc(app.name) + "</span>" +
          "</a>"
        );
      }).join("");
    }).catch(function () {});
  }

  // ---------- Reviews (giscus) ----------
  function mountReviews(site) {
    var section = document.querySelector("[data-reviews-section]");
    if (!section) return;
    var g = site.giscus || {};
    if (!(filled(g.repo) && filled(g.repoId) && filled(g.categoryId))) return; // stays hidden
    section.hidden = false;
    var s = document.createElement("script");
    s.src = "https://giscus.app/client.js";
    s.async = true;
    s.crossOrigin = "anonymous";
    var attrs = {
      "data-repo": g.repo, "data-repo-id": g.repoId, "data-category": g.category || "Reviews",
      "data-category-id": g.categoryId, "data-mapping": g.mapping || "pathname", "data-strict": "0",
      "data-reactions-enabled": "1", "data-emit-metadata": "0", "data-input-position": "top",
      "data-theme": "preferred_color_scheme", "data-lang": "en"
    };
    Object.keys(attrs).forEach(function (k) { s.setAttribute(k, attrs[k]); });
    section.querySelector("[data-reviews]").appendChild(s);
  }

  // ---------- Buy me a chai ----------
  function loadScript(src) {
    return new Promise(function (resolve, reject) {
      var s = document.createElement("script");
      s.src = src; s.async = true; s.onload = resolve; s.onerror = reject;
      document.head.appendChild(s);
    });
  }

  function buildDialog(site) {
    var sup = site.support || {};
    var owner = site.owner || "me";
    var isPhone = /Android|iPhone|iPad/i.test(navigator.userAgent || "");
    var ways = [];

    if (filled(sup.upiId)) {
      var upiLink = "upi://pay?pa=" + encodeURIComponent(sup.upiId) + "&pn=" + encodeURIComponent(sup.upiName || owner) + "&cu=INR";
      ways.push(
        '<div class="way upi">' +
          '<div class="qr" data-qr="' + esc(upiLink) + '" aria-label="UPI QR code"></div>' +
          "<div><strong>UPI</strong><small> (India)</small>" +
            '<div class="upi-id">' + esc(sup.upiId) + "</div>" +
            '<div class="upi-actions">' +
              (isPhone ? '<a class="btn chai sm" href="' + esc(upiLink) + '">Open UPI app</a>' : "") +
              '<button class="btn sm" type="button" data-copy="' + esc(sup.upiId) + '">Copy UPI ID</button>' +
            "</div>" +
          "</div>" +
        "</div>"
      );
    }
    if (filled(sup.kofi)) ways.push(link(sup.kofi, "Ko-fi", "Card or PayPal, worldwide", "One-time"));
    if (filled(sup.githubSponsors)) ways.push(link(sup.githubSponsors, "GitHub Sponsors", "For GitHub users", "Monthly or one-time"));
    if (filled(sup.patreon)) ways.push(link(sup.patreon, "Patreon", "Recurring membership", "Monthly"));

    function link(href, name, note, tag) {
      return '<a class="way" href="' + esc(href) + '" target="_blank" rel="noopener"><strong>' + esc(name) +
        "</strong><small>" + esc(note) + '</small><span class="tag">' + esc(tag) + "</span></a>";
    }

    var d = document.createElement("dialog");
    d.className = "chai-dialog";
    d.setAttribute("aria-labelledby", "chai-title");
    d.innerHTML =
      '<div class="dlg">' +
        '<div class="dlg-head"><div><h2 id="chai-title">Support ManojHustler</h2>' +
        '<p class="sub">These apps are free and always will be. Contributions help cover development time and keep new apps coming.</p></div>' +
        '<button class="x" type="button" aria-label="Close" data-close>&times;</button></div>' +
        (ways.length
          ? '<div class="ways">' + ways.join("") + "</div>"
          : '<p class="note">Support options are coming soon. Thank you for your interest.</p>') +
        '<p class="note">Recommending an app to a friend helps too.</p>' +
      "</div>";
    document.body.appendChild(d);

    d.addEventListener("click", function (e) {
      if (e.target === d || e.target.closest("[data-close]")) d.close();
      var copy = e.target.closest("[data-copy]");
      if (copy && navigator.clipboard) {
        navigator.clipboard.writeText(copy.getAttribute("data-copy")).then(function () {
          copy.textContent = "Copied";
          setTimeout(function () { copy.textContent = "Copy UPI ID"; }, 1800);
        });
      }
    });

    var qrBox = d.querySelector("[data-qr]");
    var qrDone = false;
    function drawQR() {
      if (!qrBox || qrDone) return;
      qrDone = true;
      loadScript((document.body.getAttribute("data-root") || ".") + "/js/qrcode.js").then(function () {
        var qr = window.qrcode(0, "M");
        qr.addData(qrBox.getAttribute("data-qr"));
        qr.make();
        qrBox.innerHTML = qr.createSvgTag({ cellSize: 4, margin: 0, scalable: true });
      }).catch(function () { qrBox.hidden = true; qrBox.parentNode.classList.add("no-qr"); });
    }

    each("[data-support]", function (btn) {
      btn.addEventListener("click", function (e) {
        if (typeof d.showModal !== "function") return; // old browser: follow the link
        e.preventDefault();
        d.showModal();
        drawQR();
      });
    });
  }

  function boot() {
    renderHome();
    bindDownloads();
    highlightOS();
    load("data/site.json").then(function (site) {
      buildDialog(site);
      mountReviews(site);
    }).catch(function () {});
  }

  return { pickRelease: pickRelease, boot: boot };
});
