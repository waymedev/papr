/**
 * Papr content script.
 *
 * Runs in every page. It reads the `<link rel="alternate">` tags off the DOM,
 * hands them (plus the page URL and HTML) to the pure `PaprDetect` module,
 * and reports the result to the background service worker so it can paint the
 * toolbar badge. It also answers `get-feeds` requests from the popup.
 *
 * `detect.js` is listed before this file in the manifest's `content_scripts`,
 * so `globalThis.PaprDetect` is already defined here.
 */
(function () {
  "use strict";

  /** Snapshot the page's `<link>` tags as plain descriptors. */
  function readLinks() {
    const out = [];
    const nodes = document.querySelectorAll('link[rel~="alternate"]');
    for (const el of nodes) {
      out.push({
        rel: el.getAttribute("rel") || "",
        type: el.getAttribute("type") || "",
        href: el.getAttribute("href") || "",
        title: el.getAttribute("title") || "",
      });
    }
    return out;
  }

  /** Run detection against the current document. */
  function detect() {
    return PaprDetect.detectFeeds({
      pageUrl: location.href,
      links: readLinks(),
      // documentElement.outerHTML lets the YouTube vanity-URL path resolve a
      // channel id without an extra network request.
      pageHtml: document.documentElement
        ? document.documentElement.outerHTML
        : "",
    });
  }

  /** Tell the background worker how many feeds this page has. */
  function report() {
    try {
      chrome.runtime.sendMessage({
        type: "feeds-detected",
        count: detect().length,
      });
    } catch (_) {
      /* the service worker may be asleep — harmless */
    }
  }

  // The popup asks the active tab for its feeds on open.
  chrome.runtime.onMessage.addListener(function (msg, _sender, sendResponse) {
    if (msg && msg.type === "get-feeds") {
      sendResponse({ feeds: detect(), pageUrl: location.href });
    }
    return true;
  });

  // Report once now, and again if the page mutates its <head> (SPAs).
  report();
  if (document.head) {
    const observer = new MutationObserver(report);
    observer.observe(document.head, { childList: true, subtree: true });
  }
})();
