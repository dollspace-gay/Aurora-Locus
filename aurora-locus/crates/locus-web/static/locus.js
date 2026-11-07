// Live status pages: when the server says the page changed, fetch it again
// and swap in its live parts (cards and tables), leaving forms alone.
// Without this script the pages work the same, refreshed by reloading.
(function () {
  "use strict";
  var main = document.querySelector("main[data-live]");
  if (!main || !window.EventSource || !window.fetch || !window.DOMParser) {
    return;
  }
  var busy = false;
  function refresh() {
    if (busy) {
      return;
    }
    busy = true;
    fetch(window.location.href, { credentials: "same-origin", cache: "no-store" })
      .then(function (r) {
        return r.ok ? r.text() : null;
      })
      .then(function (text) {
        if (!text) {
          return;
        }
        var doc = new DOMParser().parseFromString(text, "text/html");
        var parts = doc.querySelectorAll("[data-live-part]");
        for (var i = 0; i < parts.length; i++) {
          var old = document.getElementById(parts[i].id);
          if (old) {
            old.replaceWith(document.importNode(parts[i], true));
          }
        }
      })
      .catch(function () {})
      .then(function () {
        busy = false;
      });
  }
  var events = new EventSource(main.getAttribute("data-live"));
  events.addEventListener("change", refresh);
  events.addEventListener("end", function () {
    events.close();
  });
})();
