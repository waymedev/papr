// Global UI state. Server data (feeds, articles…) lives in React Query;
// this store holds view selection plus the appearance preferences the
// design's settings / tweaks controls drive.

import { create } from "zustand";
import i18n from "./i18n";
import type { ArticleQuery } from "./types";

export type Theme = "light" | "dark";
export type Accent = "clay" | "pine" | "indigo" | "ink";
export type Density = "compact" | "cozy" | "spacious";
export type ViewMode = "list" | "card";
export type StartupView = "all" | "unread" | "starred" | "last";

/** Behavioural preferences driven by the Settings panel. */
export interface Prefs {
  showSidebarCounts: boolean;
  showCardThumbs: boolean;
  reduceMotion: boolean;
  showReadingTime: boolean;
  markReadOnOpen: boolean;
  markReadOnScroll: boolean;
  startupView: StartupView;
  hideReadOnStartup: boolean;
}

const ls = {
  get: (k: string, fallback: string) => localStorage.getItem(k) ?? fallback,
  /** A persisted enum value, validated against `allowed` — a corrupt or
   *  stale value falls back instead of flowing through an unchecked cast. */
  oneOf: <T extends string>(k: string, allowed: readonly T[], fallback: T): T => {
    const v = localStorage.getItem(k);
    return v != null && (allowed as readonly string[]).includes(v)
      ? (v as T)
      : fallback;
  },
  num: (k: string, fallback: number) => {
    const v = localStorage.getItem(k);
    if (v == null) return fallback;
    const n = Number(v);
    // Guard against a corrupt non-numeric value — NaN would otherwise reach
    // a CSS variable (e.g. `--reader-size: NaNpx`) and break the layout.
    return Number.isFinite(n) ? n : fallback;
  },
  bool: (k: string, fallback: boolean) => {
    const v = localStorage.getItem(k);
    return v == null ? fallback : v === "1";
  },
  set: (k: string, v: string | number | boolean) =>
    localStorage.setItem(k, typeof v === "boolean" ? (v ? "1" : "0") : String(v)),
};

interface UiState {
  /** The active sidebar selection driving the article list. */
  query: ArticleQuery;
  /** Human-readable label of the current selection (list header). */
  queryLabel: string;
  /** Currently open article, or null. */
  selectedArticleId: number | null;
  /** Hide already-read articles in the list. */
  unreadOnly: boolean;
  /** Sort the list oldest-first instead of newest-first. */
  sortOldest: boolean;
  /** Live keyword search text. */
  search: string;

  // appearance preferences
  theme: Theme;
  accent: Accent;
  density: Density;
  viewMode: ViewMode;
  useSerif: boolean;
  readerSize: number;
  readerLeading: number;
  readerWidth: number;

  // behavioural preferences
  prefs: Prefs;

  // transient view modes
  focusMode: boolean;
  aiOpen: boolean;

  select: (query: ArticleQuery, label: string) => void;
  openArticle: (id: number | null) => void;
  toggleUnreadOnly: () => void;
  toggleSort: () => void;
  setSearch: (s: string) => void;

  setTheme: (t: Theme) => void;
  setAccent: (a: Accent) => void;
  setDensity: (d: Density) => void;
  setViewMode: (v: ViewMode) => void;
  setUseSerif: (v: boolean) => void;
  setReader: (p: Partial<Pick<UiState, "readerSize" | "readerLeading" | "readerWidth">>) => void;

  setPref: (patch: Partial<Prefs>) => void;

  setFocusMode: (v: boolean) => void;
  setAiOpen: (v: boolean) => void;
}

const PREF_KEYS: (keyof Prefs)[] = [
  "showSidebarCounts",
  "showCardThumbs",
  "reduceMotion",
  "showReadingTime",
  "markReadOnOpen",
  "markReadOnScroll",
  "startupView",
  "hideReadOnStartup",
];

function loadPrefs(): Prefs {
  return {
    showSidebarCounts: ls.bool("pref.showSidebarCounts", true),
    showCardThumbs: ls.bool("pref.showCardThumbs", true),
    reduceMotion: ls.bool("pref.reduceMotion", false),
    showReadingTime: ls.bool("pref.showReadingTime", true),
    markReadOnOpen: ls.bool("pref.markReadOnOpen", true),
    markReadOnScroll: ls.bool("pref.markReadOnScroll", false),
    startupView: ls.oneOf<StartupView>(
      "pref.startupView",
      ["all", "unread", "starred", "last"],
      "unread",
    ),
    hideReadOnStartup: ls.bool("pref.hideReadOnStartup", false),
  };
}

export const useUi = create<UiState>((set) => ({
  query: { kind: "all" },
  queryLabel: i18n.t("smart.all"),
  selectedArticleId: null,
  unreadOnly: false,
  sortOldest: false,
  search: "",

  theme: ls.oneOf<Theme>("theme", ["light", "dark"], "light"),
  accent: ls.oneOf<Accent>("accent", ["clay", "pine", "indigo", "ink"], "clay"),
  density: ls.oneOf<Density>(
    "density",
    ["compact", "cozy", "spacious"],
    "cozy",
  ),
  viewMode: ls.oneOf<ViewMode>("viewMode", ["list", "card"], "list"),
  useSerif: ls.bool("useSerif", true),
  readerSize: ls.num("readerSize", 17),
  readerLeading: ls.num("readerLeading", 165),
  readerWidth: ls.num("readerWidth", 680),

  prefs: loadPrefs(),

  focusMode: false,
  aiOpen: false,

  select: (query, label) => {
    // Remember the selection so the "open on startup: last view" preference
    // can restore it next launch.
    ls.set("lastView", JSON.stringify({ query, label }));
    set({ query, queryLabel: label, selectedArticleId: null, search: "" });
  },
  openArticle: (id) => set({ selectedArticleId: id }),
  toggleUnreadOnly: () => set((s) => ({ unreadOnly: !s.unreadOnly })),
  toggleSort: () => set((s) => ({ sortOldest: !s.sortOldest })),
  setSearch: (search) => set({ search }),

  setTheme: (theme) => { ls.set("theme", theme); set({ theme }); },
  setAccent: (accent) => { ls.set("accent", accent); set({ accent }); },
  setDensity: (density) => { ls.set("density", density); set({ density }); },
  setViewMode: (viewMode) => { ls.set("viewMode", viewMode); set({ viewMode }); },
  setUseSerif: (useSerif) => { ls.set("useSerif", useSerif); set({ useSerif }); },
  setReader: (p) => {
    if (p.readerSize != null) ls.set("readerSize", p.readerSize);
    if (p.readerLeading != null) ls.set("readerLeading", p.readerLeading);
    if (p.readerWidth != null) ls.set("readerWidth", p.readerWidth);
    set(p);
  },

  setPref: (patch) => {
    for (const k of PREF_KEYS) {
      if (patch[k] !== undefined) ls.set(`pref.${k}`, patch[k] as string | boolean);
    }
    set((s) => ({ prefs: { ...s.prefs, ...patch } }));
  },

  setFocusMode: (focusMode) => set({ focusMode }),
  setAiOpen: (aiOpen) => set({ aiOpen }),
}));
