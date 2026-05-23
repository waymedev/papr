// Readwise Reader configuration helpers — pure data + small mapping
// utilities. Kept React-free so the constants below can be unit-tested in
// the node-only vitest environment (see `readwise.test.ts`).

import type { ReadwiseCategory } from "../api";

// The Reader API's `category` filter values, mirroring the 9 categories the
// `GET /api/v3/list/?category=` endpoint accepts. Order here matches the
// ordering used in the Readwise documentation so the dropdown reads the
// same way users see it on readwise.io.
export const READWISE_CATEGORIES: readonly ReadwiseCategory[] = [
  "article",
  "email",
  "rss",
  "highlight",
  "note",
  "pdf",
  "epub",
  "tweet",
  "video",
] as const;

// `null` means "no category filter" — i.e. pull all categories. Stored as
// an empty string in the settings DB (the settings table only holds
// strings) and surfaced to the rest of the app as `null`.
export function parseReadwiseCategory(
  v: string | null | undefined,
): ReadwiseCategory | null {
  if (v && (READWISE_CATEGORIES as readonly string[]).includes(v)) {
    return v as ReadwiseCategory;
  }
  // Empty / null / unknown all map to "no filter". The previous build of
  // this setting stored Reader *location* values (`new`/`later`/...);
  // those land here too and get silently discarded, which is the documented
  // migration path for CWM-46.
  return null;
}

// The i18n key for each category's display label. Kept here (not in the
// component) so the test can verify every category resolves to a key
// without instantiating React.
export function readwiseCategoryLabelKey(cat: ReadwiseCategory): string {
  return `settings.sync.readwise.category.${cat}`;
}

// The synthetic feed URL the v15 migration inserts. Exported so the
// frontend can identify the Readwise feed row by URL when needed without
// hard-coding the string in multiple places.
export const READWISE_FEED_URL = "readwise://reader/later";

// Settings-DB keys for the user's last-used Reader options. Both are
// optional — the UI falls back to sensible defaults when missing. The
// category key replaces the older `readwise_reader_location` key (CWM-46);
// the old key's value, if present, is intentionally ignored on read so
// stale `new`/`later`/... values don't get sent to the API as categories.
export const READWISE_CATEGORY_SETTING = "readwise_reader_category";
export const READWISE_WITH_HTML_SETTING = "readwise_reader_with_html";
