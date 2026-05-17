// Highlight & annotation layer for the Reader (feature F7).
//
// Owns three pieces of UI laid over the article body:
//   1. a floating colour toolbar shown when text is selected,
//   2. a popover for editing / deleting an existing highlight,
//   3. an export menu (Markdown copy/save, Obsidian, Readwise, Notion).
//
// The pure re-anchoring lives in `lib/anchor.ts`; the DOM wrapping in
// `lib/highlightDom.ts`. This component is the glue that calls them and the
// `create_highlight` / export Tauri commands.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../api";
import { errorText } from "../lib/errors";
import {
  applyHighlights,
  clearHighlights,
  plainText,
  selectionAnchor,
} from "../lib/highlightDom";
import { captureContext } from "../lib/anchor";
import { HIGHLIGHT_COLORS } from "../lib/highlightColors";
import type { Highlight } from "../types";
import Icon from "./Icon";

interface Props {
  /** The article whose body is highlighted. */
  articleId: number;
  /** Ref to the rendered article-body div the highlights are applied into. */
  bodyRef: React.RefObject<HTMLDivElement | null>;
  /** Bumped by the Reader whenever the body HTML changes (extract toggle,
   *  article switch) so highlights are re-applied to the fresh DOM. */
  bodyVersion: number;
  onToast: (msg: string) => void;
}

/** The floating colour toolbar's anchor point (viewport coordinates). */
interface ToolbarPos {
  x: number;
  y: number;
}

/** The currently-selected text pending a highlight. */
interface PendingSelection {
  quote: string;
  textOffset: number;
}

export default function HighlightLayer({
  articleId,
  bodyRef,
  bodyVersion,
  onToast,
}: Props) {
  const { t } = useTranslation();
  const [highlights, setHighlights] = useState<Highlight[]>([]);
  const [toolbar, setToolbar] = useState<ToolbarPos | null>(null);
  const pendingRef = useRef<PendingSelection | null>(null);
  // The highlight whose edit popover is open, plus where to anchor it.
  const [editing, setEditing] = useState<{ hl: Highlight; x: number; y: number } | null>(
    null,
  );
  const [exportOpen, setExportOpen] = useState(false);

  // Load the article's stored highlights.
  const reload = () => {
    api
      .listHighlights(articleId)
      .then(setHighlights)
      .catch(() => setHighlights([]));
  };
  useEffect(() => {
    setHighlights([]);
    reload();
    setEditing(null);
    setExportOpen(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [articleId]);

  // Re-apply highlights to the body whenever the highlight set or the body
  // HTML changes. `bodyVersion` flips when the Reader swaps the body markup.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    if (highlights.length === 0) {
      clearHighlights(el);
      return;
    }
    applyHighlights(el, highlights);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlights, bodyVersion]);

  // Show the colour toolbar when the user finishes a selection in the body.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const onMouseUp = () => {
      // Defer so the browser has committed the selection.
      window.setTimeout(() => {
        const sel = selectionAnchor(el);
        if (!sel) {
          pendingRef.current = null;
          setToolbar(null);
          return;
        }
        pendingRef.current = sel;
        const range = window.getSelection()?.getRangeAt(0);
        const rect = range?.getBoundingClientRect();
        if (rect) {
          setToolbar({ x: rect.left + rect.width / 2, y: rect.top });
        }
      }, 0);
    };
    el.addEventListener("mouseup", onMouseUp);
    return () => el.removeEventListener("mouseup", onMouseUp);
  }, [bodyRef]);

  // Clicking an existing highlight <mark> opens its edit popover.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const onClick = (e: MouseEvent) => {
      const mark = (e.target as HTMLElement).closest("mark[data-hl]");
      if (!mark) return;
      e.preventDefault();
      e.stopPropagation();
      const id = Number((mark as HTMLElement).dataset.hl);
      const hl = highlights.find((h) => h.id === id);
      if (!hl) return;
      const r = mark.getBoundingClientRect();
      setEditing({ hl, x: r.left, y: r.bottom + 6 });
    };
    el.addEventListener("click", onClick, true);
    return () => el.removeEventListener("click", onClick, true);
  }, [bodyRef, highlights]);

  const createHighlight = async (color: string) => {
    const el = bodyRef.current;
    const pending = pendingRef.current;
    if (!el || !pending) return;
    const ctx = captureContext(plainText(el), pending.textOffset, pending.textOffset + pending.quote.length);
    try {
      await api.createHighlight({
        articleId,
        quote: pending.quote,
        prefix: ctx.prefix,
        suffix: ctx.suffix,
        textOffset: pending.textOffset,
        color,
        note: "",
      });
      window.getSelection()?.removeAllRanges();
      setToolbar(null);
      pendingRef.current = null;
      reload();
    } catch (e) {
      onToast(errorText(e));
    }
  };

  return (
    <>
      {toolbar && (
        <SelectionToolbar
          pos={toolbar}
          onPick={createHighlight}
          onDismiss={() => setToolbar(null)}
        />
      )}
      {editing && (
        <HighlightPopover
          key={editing.hl.id}
          hl={editing.hl}
          x={editing.x}
          y={editing.y}
          onClose={() => setEditing(null)}
          onChanged={reload}
          onToast={onToast}
        />
      )}
      <div className="hl-export-wrap">
        <button
          className="tb-btn"
          title={t("highlights.exportTitle")}
          aria-label={t("highlights.exportTitle")}
          onClick={() => setExportOpen((v) => !v)}
          aria-haspopup="menu"
          aria-expanded={exportOpen}
        >
          <Icon name="share" size={16} />
        </button>
        {exportOpen && (
          <ExportMenu
            articleId={articleId}
            count={highlights.length}
            onClose={() => setExportOpen(false)}
            onToast={onToast}
          />
        )}
      </div>
    </>
  );
}

/* ── floating colour toolbar ─────────────────────────────────── */
function SelectionToolbar({
  pos,
  onPick,
  onDismiss,
}: {
  pos: ToolbarPos;
  onPick: (color: string) => void;
  onDismiss: () => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState({ left: pos.x, top: pos.y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let left = pos.x - r.width / 2;
    let top = pos.y - r.height - 8;
    left = Math.max(8, Math.min(left, window.innerWidth - r.width - 8));
    if (top < 8) top = pos.y + 22; // flip below the selection if clipped
    setPlace({ left, top });
  }, [pos]);

  // A scroll or an outside click ends the selection toolbar.
  useEffect(() => {
    const onScroll = () => onDismiss();
    window.addEventListener("scroll", onScroll, true);
    return () => window.removeEventListener("scroll", onScroll, true);
  }, [onDismiss]);

  return (
    <div
      ref={ref}
      className="hl-toolbar"
      role="toolbar"
      aria-label={t("highlights.toolbarLabel")}
      style={{ left: place.left, top: place.top }}
      // Keep the text selection alive while the toolbar is clicked.
      onMouseDown={(e) => e.preventDefault()}
    >
      {HIGHLIGHT_COLORS.map((c) => (
        <button
          key={c.key}
          className="hl-swatch"
          style={{ background: c.swatch }}
          title={t(`highlights.color.${c.key}`)}
          aria-label={t(`highlights.color.${c.key}`)}
          onClick={() => onPick(c.key)}
        />
      ))}
    </div>
  );
}

/* ── existing-highlight edit popover ─────────────────────────── */
function HighlightPopover({
  hl,
  x,
  y,
  onClose,
  onChanged,
  onToast,
}: {
  hl: Highlight;
  x: number;
  y: number;
  onClose: () => void;
  onChanged: () => void;
  onToast: (m: string) => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const [note, setNote] = useState(hl.note);
  const [place, setPlace] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let left = x;
    let top = y;
    if (left + r.width > window.innerWidth - 8) left = window.innerWidth - r.width - 8;
    if (top + r.height > window.innerHeight - 8) top = y - r.height - 28;
    setPlace({ left: Math.max(8, left), top: Math.max(8, top) });
  }, [x, y]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) save();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const tm = window.setTimeout(() => {
      document.addEventListener("mousedown", onDown);
      window.addEventListener("keydown", onKey);
    }, 0);
    return () => {
      window.clearTimeout(tm);
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [note]);

  const save = async () => {
    if (note !== hl.note) {
      try {
        await api.updateHighlightNote(hl.id, note);
        onChanged();
      } catch (e) {
        onToast(errorText(e));
      }
    }
    onClose();
  };

  const recolor = async (color: string) => {
    try {
      await api.setHighlightColor(hl.id, color);
      onChanged();
    } catch (e) {
      onToast(errorText(e));
    }
  };

  const remove = async () => {
    try {
      await api.deleteHighlight(hl.id);
      onChanged();
      onClose();
    } catch (e) {
      onToast(errorText(e));
    }
  };

  return (
    <div
      ref={ref}
      className="hl-popover"
      role="dialog"
      aria-label={t("highlights.editTitle")}
      style={{ left: place.left, top: place.top }}
    >
      <blockquote className="hl-quote">{hl.quote}</blockquote>
      <div className="hl-swatch-row">
        {HIGHLIGHT_COLORS.map((c) => (
          <button
            key={c.key}
            className={`hl-swatch ${hl.color === c.key ? "active" : ""}`}
            style={{ background: c.swatch }}
            title={t(`highlights.color.${c.key}`)}
            aria-label={t(`highlights.color.${c.key}`)}
            onClick={() => recolor(c.key)}
          />
        ))}
      </div>
      <textarea
        className="hl-note-input"
        placeholder={t("highlights.notePlaceholder")}
        value={note}
        autoFocus
        onChange={(e) => setNote(e.target.value)}
      />
      <div className="hl-popover-actions">
        <button className="s-btn danger" onClick={remove}>
          <Icon name="trash" size={12} /> {t("common.delete")}
        </button>
        <button className="s-btn primary" onClick={save}>
          {t("common.done")}
        </button>
      </div>
    </div>
  );
}

/* ── export menu ─────────────────────────────────────────────── */
function ExportMenu({
  articleId,
  count,
  onClose,
  onToast,
}: {
  articleId: number;
  count: number;
  onClose: () => void;
  onToast: (m: string) => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const tm = window.setTimeout(() => {
      document.addEventListener("mousedown", onDown);
      window.addEventListener("keydown", onKey);
    }, 0);
    return () => {
      window.clearTimeout(tm);
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  /** Run an export action, guarding against the no-highlights case. */
  const run = async (fn: () => Promise<void>) => {
    if (count === 0) {
      onToast(t("highlights.exportNothing"));
      onClose();
      return;
    }
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      onToast(errorText(e));
    } finally {
      setBusy(false);
      onClose();
    }
  };

  const copyMd = () =>
    run(async () => {
      const md = await api.exportHighlightsMarkdown(articleId);
      await navigator.clipboard.writeText(md);
      onToast(t("highlights.copiedMarkdown"));
    });

  const saveMd = () =>
    run(async () => {
      const md = await api.exportHighlightsMarkdown(articleId);
      // No file-dialog plugin is bundled; offer the document as a download
      // through the webview, which the user can place anywhere.
      const blob = new Blob([md], { type: "text/markdown" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `highlights-${articleId}.md`;
      a.click();
      URL.revokeObjectURL(url);
      onToast(t("highlights.savedMarkdown"));
    });

  const toObsidian = () =>
    run(async () => {
      const path = await api.exportHighlightsToObsidian(articleId);
      onToast(t("highlights.savedObsidian", { path }));
    });

  const toReadwise = () =>
    run(async () => {
      const n = await api.exportHighlightsToReadwise(articleId);
      onToast(t("highlights.sentReadwise", { count: n }));
    });

  const toNotion = () =>
    run(async () => {
      const n = await api.exportHighlightsToNotion(articleId);
      onToast(t("highlights.sentNotion", { count: n }));
    });

  return (
    <div ref={ref} className="ctx-menu hl-export-menu" role="menu">
      <div className="hl-export-head">
        {t("highlights.exportHeading", { count })}
      </div>
      <button className="ctx-item" role="menuitem" onClick={copyMd} disabled={busy}>
        <Icon name="copy" size={13} /> {t("highlights.copyMarkdown")}
      </button>
      <button className="ctx-item" role="menuitem" onClick={saveMd} disabled={busy}>
        <Icon name="text" size={13} /> {t("highlights.saveMarkdown")}
      </button>
      <button className="ctx-item" role="menuitem" onClick={toObsidian} disabled={busy}>
        <Icon name="folder" size={13} /> {t("highlights.exportObsidian")}
      </button>
      <button className="ctx-item" role="menuitem" onClick={toReadwise} disabled={busy}>
        <Icon name="bookmark" size={13} /> {t("highlights.exportReadwise")}
      </button>
      <button className="ctx-item" role="menuitem" onClick={toNotion} disabled={busy}>
        <Icon name="list" size={13} /> {t("highlights.exportNotion")}
      </button>
    </div>
  );
}
