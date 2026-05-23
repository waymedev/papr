// Pure-logic tests for the Readwise Reader settings helpers (CWM-37 step 4).
// Vitest runs in a node environment (see vitest.config.ts), so this file
// stays DOM-free — it covers the constants, the location-string sanitiser,
// and the i18n key shape that the Settings panel consumes.

import { describe, it, expect } from "vitest";
import type { ReadwiseLocation } from "./../api";
import type { SourceType } from "./../types";
import {
  DEFAULT_READWISE_LOCATION,
  READWISE_FEED_URL,
  READWISE_LOCATIONS,
  READWISE_LOCATION_SETTING,
  READWISE_WITH_HTML_SETTING,
  parseReadwiseLocation,
  readwiseLocationLabelKey,
} from "./readwise";

describe("READWISE_LOCATIONS", () => {
  it("matches the five Reader API buckets in display order", () => {
    expect(READWISE_LOCATIONS).toEqual([
      "new",
      "later",
      "shortlist",
      "archive",
      "feed",
    ]);
  });

  it("includes the default location", () => {
    expect(READWISE_LOCATIONS).toContain(DEFAULT_READWISE_LOCATION);
  });
});

describe("parseReadwiseLocation", () => {
  it("returns the input when it is a valid location", () => {
    for (const loc of READWISE_LOCATIONS) {
      expect(parseReadwiseLocation(loc)).toBe(loc);
    }
  });

  it("falls back to the default when null / empty / unknown", () => {
    expect(parseReadwiseLocation(null)).toBe(DEFAULT_READWISE_LOCATION);
    expect(parseReadwiseLocation(undefined)).toBe(DEFAULT_READWISE_LOCATION);
    expect(parseReadwiseLocation("")).toBe(DEFAULT_READWISE_LOCATION);
    expect(parseReadwiseLocation("bogus")).toBe(DEFAULT_READWISE_LOCATION);
    // A capitalised value from an older build must not slip through —
    // the Reader API only accepts the lowercase form.
    expect(parseReadwiseLocation("Later")).toBe(DEFAULT_READWISE_LOCATION);
  });
});

describe("readwiseLocationLabelKey", () => {
  it("produces a stable i18n key per location", () => {
    expect(readwiseLocationLabelKey("later")).toBe(
      "settings.sync.readwise.location.later",
    );
  });

  it("produces a distinct key for every location", () => {
    const keys = READWISE_LOCATIONS.map(readwiseLocationLabelKey);
    expect(new Set(keys).size).toBe(READWISE_LOCATIONS.length);
  });
});

describe("settings keys", () => {
  it("use the readwise_reader_ prefix so they don't collide with the highlights integration's readwise_token", () => {
    expect(READWISE_LOCATION_SETTING).toBe("readwise_reader_location");
    expect(READWISE_WITH_HTML_SETTING).toBe("readwise_reader_with_html");
    // Token is owned by the highlights integration; we must not invent a
    // second key for it here — see readwise_reader.rs::TOKEN_SETTING.
    expect(READWISE_LOCATION_SETTING).not.toBe("readwise_token");
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

describe("ReadwiseLocation", () => {
  // Same compile-time guard for the API location union — adding a value
  // to the Reader API (e.g. a future "highlights" bucket) means updating
  // the union and READWISE_LOCATIONS together. The runtime assertions
  // also keep tsc happy that the constant array is assignable.
  it("admits every value in READWISE_LOCATIONS", () => {
    for (const loc of READWISE_LOCATIONS) {
      const v: ReadwiseLocation = loc;
      expect(typeof v).toBe("string");
    }
  });
});
