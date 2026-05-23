// Readwise Reader configuration helpers — pure data + small mapping
// utilities. Kept React-free so the constants below can be unit-tested in
// the node-only vitest environment (see `readwise.test.ts`).

import type { ReadwiseLocation } from "../api";

// The Reader API's `location` filter — the same five buckets the user picks
// from in the Readwise web UI. Order here matches the visual ordering of
// the Reader sidebar so the dropdown reads top-to-bottom the same way.
export const READWISE_LOCATIONS: readonly ReadwiseLocation[] = [
  "new",
  "later",
  "shortlist",
  "archive",
  "feed",
] as const;

// Anything stored in the settings DB started life as user input or as a
// previous version of this code — neither is guaranteed to still be a
// valid `ReadwiseLocation`. Normalise on read so a stale value falls back
// to the default bucket instead of getting passed straight to the API.
export const DEFAULT_READWISE_LOCATION: ReadwiseLocation = "later";

export function parseReadwiseLocation(v: string | null | undefined): ReadwiseLocation {
  if (v && (READWISE_LOCATIONS as readonly string[]).includes(v)) {
    return v as ReadwiseLocation;
  }
  return DEFAULT_READWISE_LOCATION;
}

// The i18n key for each location's display label. Kept here (not in the
// component) so the test can verify every location resolves to a key
// without instantiating React.
export function readwiseLocationLabelKey(loc: ReadwiseLocation): string {
  return `settings.sync.readwise.location.${loc}`;
}

// The synthetic feed URL the v15 migration inserts. Exported so the
// frontend can identify the Readwise feed row by URL when needed without
// hard-coding the string in multiple places.
export const READWISE_FEED_URL = "readwise://reader/later";

// Settings-DB keys for the user's last-used Reader options. Both are
// optional — the UI falls back to sensible defaults when missing.
export const READWISE_LOCATION_SETTING = "readwise_reader_location";
export const READWISE_WITH_HTML_SETTING = "readwise_reader_with_html";
