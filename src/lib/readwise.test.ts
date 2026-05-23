// Pure-logic tests for the Readwise Reader settings helpers (CWM-37 step 4,
// updated for CWM-46 — `category` replaces `location` in the Settings UI).
// Vitest runs in a node environment (see vitest.config.ts), so this file
// stays DOM-free.

import { describe, it, expect } from "vitest";
import type { ReadwiseCategory } from "./../api";
import type { SourceType } from "./../types";
import {
  READWISE_FEED_URL,
  READWISE_CATEGORIES,
  READWISE_CATEGORY_SETTING,
  READWISE_WITH_HTML_SETTING,
  parseReadwiseCategory,
  readwiseCategoryLabelKey,
} from "./readwise";

describe("READWISE_CATEGORIES", () => {
  it("matches the nine Reader API category values", () => {
    expect(READWISE_CATEGORIES).toEqual([
      "article",
      "email",
      "rss",
      "highlight",
      "note",
      "pdf",
      "epub",
      "tweet",
      "video",
    ]);
  });
});

describe("parseReadwiseCategory", () => {
  it("returns the input when it is a valid category", () => {
    for (const cat of READWISE_CATEGORIES) {
      expect(parseReadwiseCategory(cat)).toBe(cat);
    }
  });

  it("returns null for null / empty / unknown values", () => {
    expect(parseReadwiseCategory(null)).toBeNull();
    expect(parseReadwiseCategory(undefined)).toBeNull();
    expect(parseReadwiseCategory("")).toBeNull();
    expect(parseReadwiseCategory("bogus")).toBeNull();
    // A capitalised value must not slip through — the Reader API only
    // accepts the lowercase form.
    expect(parseReadwiseCategory("Article")).toBeNull();
  });

  it("discards stale Reader location values from older builds", () => {
    // CWM-46: the old `readwise_reader_location` setting stored values like
    // `new` / `later` / `shortlist` / `archive` / `feed`. They are not
    // valid categories under the new field semantics and must be dropped.
    for (const loc of ["new", "later", "shortlist", "archive", "feed"]) {
      expect(parseReadwiseCategory(loc)).toBeNull();
    }
  });
});

describe("readwiseCategoryLabelKey", () => {
  it("produces a stable i18n key per category", () => {
    expect(readwiseCategoryLabelKey("article")).toBe(
      "settings.sync.readwise.category.article",
    );
  });

  it("produces a distinct key for every category", () => {
    const keys = READWISE_CATEGORIES.map(readwiseCategoryLabelKey);
    expect(new Set(keys).size).toBe(READWISE_CATEGORIES.length);
  });
});

describe("settings keys", () => {
  it("use the readwise_reader_ prefix so they don't collide with the highlights integration's readwise_token", () => {
    expect(READWISE_CATEGORY_SETTING).toBe("readwise_reader_category");
    expect(READWISE_WITH_HTML_SETTING).toBe("readwise_reader_with_html");
    // Token is owned by the highlights integration; we must not invent a
    // second key for it here — see readwise_reader.rs::TOKEN_SETTING.
    expect(READWISE_CATEGORY_SETTING).not.toBe("readwise_token");
  });
});

describe("READWISE_FEED_URL", () => {
  it("matches the synthetic feed inserted by the v15 DB migration", () => {
    // Hard-coded in src-tauri/src/db.rs (v15 migration). If the migration
    // changes the URL, this constant — and this assertion — must follow.
    expect(READWISE_FEED_URL).toBe("readwise://reader/later");
  });
});

describe("SourceType", () => {
  // The synthetic Readwise feed is rendered through the same
  // `feed.sourceType` plumbing as RSS / YouTube / etc., so the union must
  // include "readwise". This test fails to compile if step 1's type
  // addition is ever reverted, which is exactly what we want.
  it("includes 'readwise' as a valid SourceType", () => {
    const rw: SourceType = "readwise";
    expect(rw).toBe("readwise");
  });
});

describe("ReadwiseCategory", () => {
  // Compile-time guard for the API category union — adding a value to the
  // Reader API means updating the union and READWISE_CATEGORIES together.
  // The runtime assertions also keep tsc happy that the constant array is
  // assignable.
  it("admits every value in READWISE_CATEGORIES", () => {
    for (const cat of READWISE_CATEGORIES) {
      const v: ReadwiseCategory = cat;
      expect(typeof v).toBe("string");
    }
  });
});
