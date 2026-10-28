(function (root, factory) {
  var api = factory();
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof document !== "undefined") api.boot();
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  var REPO = "manoj7019/cutdeck-desktop";

  function esc(value) {
    return String(value == null ? "" : value).replace(/[&<>"']/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch];
    });
  }

  // Newest release whose tag starts with prefix and which actually has assetName.
  // releases/latest can only name one release for the whole repo, so Cutdeck (v*)
  // and Bob Player (bobplayer-v*) are resolved separately. The list is newest first.
  function pickRelease(releases, prefix, assetName) {
    if (!prefix || !assetName || !Array.isArray(releases)) return "";
    for (var i = 0; i < releases.length; i++) {
      var rel = releases[i] || {};
      var tag = String(rel.tag_name || "");
      if (tag.indexOf(prefix) !== 0) continue;
      var assets = rel.assets || [];
      for (var j = 0; j < assets.length; j++) {
        var asset = assets[j] || {};
        if (asset.name === assetName && asset.browser_download_url) return asset.browser_download_url;
      }
    }
    return "";
  }

  function closestAttr(node, name) {
    var cur = node;
    while (cur && cur.getAttribute) {
      var value = cur.getAttribute(name);
      if (value) return value;
      cur = cur.parentElement;
    }
    return "";
  }

  function load(path) {
    var root = document.body.getAttribute("data-root") || ".";
    return fetch(root + "/" + path).then(function (res) {
      if (!res.ok) throw new Error(path);
      return res.json();
    });
  }

  function configured(value) {
    return !!value && String(value).indexOf("REPLACE_ME") === -1;
  }

  function applySupport(site) {
    var url = site.supportUrl || "";
    var label = site.supportLabel || "Support my work";
    var placeholder = !configured(url);
    Array.prototype.forEach.call(document.querySelectorAll("[data-support]"), function (node) {
      if (url) node.href = url;
      node.textContent = label;
    });
    Array.prototype.forEach.call(document.querySelectorAll("[data-support-blurb]"), function (node) {
      if (site.supportBlurb) node.textContent = site.supportBlurb;
    });
    Array.prototype.forEach.call(document.querySelectorAll("[data-support-note]"), function (node) {
      node.hidden = !placeholder;
    });
  }

  function mountReviews(site) {
    var host = document.querySelector("[data-reviews]");
    if (!host) return;
    var soon = host.querySelector("[data-reviews-soon]");
    var g = site.giscus || {};
    var ready = configured(g.repo) && configured(g.repoId) && configured(g.categoryId);
    if (!ready) {
      if (soon) soon.hidden = false;
      return;
    }
    if (soon) soon.hidden = true;
    var script = document.createElement("script");
    script.src = "https://giscus.app/client.js";
    script.async = true;
    script.crossOrigin = "anonymous";
    script.setAttribute("data-repo", g.repo);
    script.setAttribute("data-repo-id", g.repoId);
    script.setAttribute("data-category", g.category || "Reviews");
    script.setAttribute("data-category-id", g.categoryId);
    script.setAttribute("data-mapping", g.mapping || "pathname");
    script.setAttribute("data-strict", "0");
    script.setAttribute("data-reactions-enabled", "1");
    script.setAttribute("data-emit-metadata", "0");
    script.setAttribute("data-input-position", "top");
    script.setAttribute("data-theme", g.theme || "dark");
    script.setAttribute("data-lang", "en");
    host.appendChild(script);
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
        .then(function (res) {
          if (!res.ok) throw new Error("releases");
          return res.json();
        })
        .then(function (releases) {
          byRepo[repo].forEach(function (node) {
            var url = pickRelease(releases, closestAttr(node, "data-release-prefix"), node.getAttribute("data-release-asset"));
            if (url) node.href = url;
          });
        })
        .catch(function () {});
    });
  }

  function renderHome() {
    if (document.body.getAttribute("data-page") !== "home") return;
    var grid = document.querySelector("[data-app-grid]");
    if (!grid) return;
    var pageRoot = document.body.getAttribute("data-root") || ".";
    load("data/apps.json")
      .then(function (data) {
        var apps = data.apps || [];
        if (!apps.length) return;
        grid.innerHTML = apps
          .map(function (app) {
            var pills = (app.platforms || [])
              .map(function (name) {
                return "<span class=\"pill\">" + esc(name) + "</span>";
              })
              .join("");
            var icon = app.icon
              ? "<img src=\"" + esc(pageRoot + "/" + String(app.icon).replace(/^\//, "")) + "\" alt=\"\" width=\"64\" height=\"64\">"
              : "";
            return (
              "<a class=\"card\" href=\"" + esc(app.href) + "\">" +
                "<div class=\"card-top\">" + icon +
                  "<div><h2>" + esc(app.name) + "</h2><p>" + esc(app.tagline) + "</p></div>" +
                "</div>" +
                "<p class=\"card-desc\">" + esc(app.description) + "</p>" +
                "<div class=\"pills\">" + pills + "</div>" +
                "<span class=\"reviews-hint\">Reviews on the page</span>" +
                "<span class=\"more\">Open " + esc(app.name) + "</span>" +
              "</a>"
            );
          })
          .join("");
      })
      .catch(function () {});
  }

  function boot() {
    bindDownloads();
    load("data/site.json")
      .then(function (site) {
        applySupport(site);
        mountReviews(site);
      })
      .catch(function () {});
    renderHome();
  }

  return { pickRelease: pickRelease, boot: boot };
});
