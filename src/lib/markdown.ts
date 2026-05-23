// Render markdown produced by the AI features (summaries, Q&A, digests).
//
// The text is LLM output, but the LLM summarizes attacker-influenced feed
// content, so its markdown can carry raw HTML. `marked` does not sanitize,
// and the result is injected via dangerouslySetInnerHTML inside a Tauri
// webview — so every render is passed through an allowlist sanitizer first.

import { marked } from "marked";

marked.setOptions({ breaks: true, gfm: true });

// Elements removed wholesale, including their text content.
const DROP_TAGS = new Set([
  "script", "style", "iframe", "object", "embed", "noscript",
  "template", "link", "meta", "form", "input", "button", "svg",
]);

// Elements kept as-is. Anything else is unwrapped to its children.
const ALLOWED_TAGS = new Set([
  "p", "br", "hr", "span", "div", "blockquote", "pre", "code",
  "strong", "em", "b", "i", "u", "s", "del", "mark", "sub", "sup",
  "ul", "ol", "li", "a",
  "h1", "h2", "h3", "h4", "h5", "h6",
  "table", "thead", "tbody", "tfoot", "tr", "th", "td",
]);

// Per-tag attribute allowlist. Everything not listed (incl. on* handlers,
// style, srcset) is stripped.
const ALLOWED_ATTRS: Record<string, Set<string>> = {
  a: new Set(["href", "title"]),
  td: new Set(["colspan", "rowspan"]),
  th: new Set(["colspan", "rowspan", "scope"]),
};

const SAFE_HREF = /^(https?:|mailto:)/i;

function sanitizeElement(el: Element) {
  // Depth-first: process children before the element may be unwrapped.
  for (const child of Array.from(el.children)) sanitizeElement(child);

  const tag = el.tagName.toLowerCase();

  if (DROP_TAGS.has(tag)) {
    el.remove();
    return;
  }
  if (!ALLOWED_TAGS.has(tag)) {
    el.replaceWith(...Array.from(el.childNodes)); // unwrap, keep contents
    return;
  }

  const allowed = ALLOWED_ATTRS[tag];
  for (const attr of Array.from(el.attributes)) {
    if (!allowed || !allowed.has(attr.name.toLowerCase())) {
      el.removeAttribute(attr.name);
    }
  }
  if (tag === "a") {
    const href = (el.getAttribute("href") ?? "").trim();
    if (SAFE_HREF.test(href)) {
      el.setAttribute("rel", "noopener noreferrer nofollow");
    } else {
      el.removeAttribute("href");
    }
  }
}

export function renderMarkdown(text: string): string {
  const raw = marked.parse(text, { async: false });
  // DOMParser documents are inert — nothing here executes.
  const doc = new DOMParser().parseFromString(raw, "text/html");
  for (const child of Array.from(doc.body.children)) sanitizeElement(child);
  return doc.body.innerHTML;
}

// Reader-mode HTML returned by an external full-text provider (defuddle.md
// gives HTML; r.jina.ai gives Markdown that `marked` then turns into HTML).
// Stripped via the same DOM allowlist as `renderMarkdown` but with images,
// figures, and basic captions kept — reader-mode pages rely on those.
const READER_ALLOWED_TAGS = new Set([
  ...ALLOWED_TAGS,
  "img",
  "figure",
  "figcaption",
  "picture",
  "source",
]);
const READER_ALLOWED_ATTRS: Record<string, Set<string>> = {
  ...ALLOWED_ATTRS,
  img: new Set(["src", "alt", "title", "width", "height"]),
  source: new Set(["src", "srcset", "type", "media"]),
};
const SAFE_IMG_SRC = /^(https?:|data:image\/)/i;

function sanitizeReaderElement(el: Element) {
  for (const child of Array.from(el.children)) sanitizeReaderElement(child);
  const tag = el.tagName.toLowerCase();
  if (DROP_TAGS.has(tag)) {
    el.remove();
    return;
  }
  if (!READER_ALLOWED_TAGS.has(tag)) {
    el.replaceWith(...Array.from(el.childNodes));
    return;
  }
  const allowed = READER_ALLOWED_ATTRS[tag];
  for (const attr of Array.from(el.attributes)) {
    if (!allowed || !allowed.has(attr.name.toLowerCase())) {
      el.removeAttribute(attr.name);
    }
  }
  if (tag === "a") {
    const href = (el.getAttribute("href") ?? "").trim();
    if (SAFE_HREF.test(href)) {
      el.setAttribute("rel", "noopener noreferrer nofollow");
    } else {
      el.removeAttribute("href");
    }
  } else if (tag === "img" || tag === "source") {
    const src = (el.getAttribute("src") ?? "").trim();
    if (src && !SAFE_IMG_SRC.test(src)) el.removeAttribute("src");
  }
}

/** Render content returned by the "fetch full text" providers. Markdown is
 *  parsed through `marked`; raw HTML is passed through unchanged. Both pass
 *  through a permissive DOM allowlist (images allowed, scripts/handlers
 *  removed). */
export function renderProviderBody(text: string, kind: "html" | "markdown"): string {
  const raw = kind === "markdown" ? marked.parse(text, { async: false }) : text;
  const doc = new DOMParser().parseFromString(raw, "text/html");
  for (const child of Array.from(doc.body.children)) sanitizeReaderElement(child);
  return doc.body.innerHTML;
}
