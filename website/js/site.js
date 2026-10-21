(function () {
  var root = document.body.getAttribute("data-root") || ".";

  function load(path) {
    return fetch(root + "/" + path).then(function (res) {
      if (!res.ok) throw new Error(path);
      return res.json();
    });
  }

  function esc(value) {
    return String(value == null ? "" : value).replace(/[&<>"']/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch];
    });
  }

  load("data/site.json")
    .then(function (site) {
      var url = site.supportUrl || "";
      var label = site.supportLabel || "Buy me a coffee";
      var placeholder = !url || url.indexOf("REPLACE_ME") !== -1;
      Array.prototype.forEach.call(document.querySelectorAll("[data-support]"), function (node) {
        if (url) node.href = url;
        node.textContent = label;
      });
      Array.prototype.forEach.call(document.querySelectorAll("[data-support-note]"), function (node) {
        node.hidden = !placeholder;
      });
    })
    .catch(function () {});

  if (document.body.getAttribute("data-page") !== "home") return;
  var grid = document.querySelector("[data-app-grid]");
  if (!grid) return;

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
            ? "<img src=\"" + esc(root + "/" + String(app.icon).replace(/^\//, "")) + "\" alt=\"\" width=\"64\" height=\"64\">"
            : "";
          return (
            "<a class=\"card\" href=\"" + esc(app.href) + "\">" +
              "<div class=\"card-top\">" + icon +
                "<div><h2>" + esc(app.name) + "</h2><p>" + esc(app.tagline) + "</p></div>" +
              "</div>" +
              "<p class=\"card-desc\">" + esc(app.description) + "</p>" +
              "<div class=\"pills\">" + pills + "</div>" +
              "<span class=\"more\">Open " + esc(app.name) + "</span>" +
            "</a>"
          );
        })
        .join("");
    })
    .catch(function () {});
})();
