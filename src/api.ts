// Thin typed wrappers over the Tauri command surface (src-tauri/src/commands.rs).

import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  AiEvent,
  ArticleDetail,
  ArticleQuery,
  ArticleSummary,
  Feed,
  Folder,
  RefreshProgress,
  Rule,
  RuleAction,
  RuleField,
  RulePreview,
  SmartCounts,
  Tag,
} from "./types";

// ── folders ──
export const listFolders = () => invoke<Folder[]>("list_folders");
export const createFolder = (name: string) =>
  invoke<number>("create_folder", { name });
export const renameFolder = (id: number, name: string) =>
  invoke<void>("rename_folder", { id, name });
export const deleteFolder = (id: number) =>
  invoke<void>("delete_folder", { id });

// ── feeds ──
export const listFeeds = () => invoke<Feed[]>("list_feeds");
export const addFeed = (url: string, folderId: number | null) =>
  invoke<Feed>("add_feed", { url, folderId });
export const deleteFeed = (id: number) => invoke<void>("delete_feed", { id });
export const moveFeed = (id: number, folderId: number | null) =>
  invoke<void>("move_feed", { id, folderId });
export const renameFeed = (id: number, title: string) =>
  invoke<void>("rename_feed", { id, title });

/** Refresh all feeds, reporting progress through the supplied callback. */
export function refreshFeeds(
  onProgress?: (p: RefreshProgress) => void,
): Promise<number> {
  const channel = new Channel<RefreshProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return invoke<number>("refresh_feeds", { onProgress: channel });
}

// ── articles ──
export const listArticles = (
  query: ArticleQuery,
  unreadOnly: boolean,
  search: string | null,
  oldestFirst: boolean,
  limit: number,
  offset: number,
) =>
  invoke<ArticleSummary[]>("list_articles", {
    query,
    unreadOnly,
    search,
    oldestFirst,
    limit,
    offset,
  });

export const getArticle = (id: number) =>
  invoke<ArticleDetail>("get_article", { id });
export const markRead = (id: number, read: boolean) =>
  invoke<void>("mark_read", { id, read });
export const markStarred = (id: number, starred: boolean) =>
  invoke<void>("mark_starred", { id, starred });
export const markReadLater = (id: number, value: boolean) =>
  invoke<void>("mark_read_later", { id, value });
export const markAllRead = (query: ArticleQuery) =>
  invoke<number>("mark_all_read", { query });
export const smartCounts = () => invoke<SmartCounts>("smart_counts");

// ── full-text extraction ──
export const extractFulltext = (articleId: number) =>
  invoke<string>("extract_fulltext", { articleId });

// ── OPML ──
export const importOpml = (content: string) =>
  invoke<number>("import_opml", { content });
export const exportOpml = () => invoke<string>("export_opml");

// ── AI (streaming over a Channel) ──
export function aiSummarize(
  articleId: number,
  onToken: (e: AiEvent) => void,
): Promise<void> {
  const channel = new Channel<AiEvent>();
  channel.onmessage = onToken;
  return invoke<void>("ai_summarize", { articleId, onToken: channel });
}

export function aiAsk(
  question: string,
  onToken: (e: AiEvent) => void,
): Promise<void> {
  const channel = new Channel<AiEvent>();
  channel.onmessage = onToken;
  return invoke<void>("ai_ask", { question, onToken: channel });
}

export function aiDigest(onToken: (e: AiEvent) => void): Promise<void> {
  const channel = new Channel<AiEvent>();
  channel.onmessage = onToken;
  return invoke<void>("ai_digest", { onToken: channel });
}

// ── settings ──
export const getSetting = (key: string) =>
  invoke<string | null>("get_setting", { key });
export const setSetting = (key: string, value: string) =>
  invoke<void>("set_setting", { key, value });

// ── storage ──
export interface StorageStats {
  dbBytes: number;
  articleCount: number;
  feedCount: number;
}
export const storageStats = () => invoke<StorageStats>("storage_stats");
export const cleanupArticles = (days: number) =>
  invoke<number>("cleanup_articles", { days });
export const vacuumDb = () => invoke<void>("vacuum_db");
export const resetSettings = () => invoke<void>("reset_settings");
export const clearAllData = () => invoke<void>("clear_all_data");

// ── network ──
export const applyNetworkSettings = () =>
  invoke<void>("apply_network_settings");

// ── FreshRSS sync ──
export interface FreshRssStatus {
  connected: boolean;
  url: string | null;
}
export const freshrssStatus = () => invoke<FreshRssStatus>("freshrss_status");
export const freshrssConnect = (
  url: string,
  username: string,
  password: string,
) => invoke<void>("freshrss_connect", { url, username, password });
export const freshrssDisconnect = () => invoke<void>("freshrss_disconnect");
export const freshrssSync = () => invoke<number>("freshrss_sync");

// ── tray ──
export const refreshTray = () => invoke<void>("refresh_tray");

// ── tags ──
export const listTags = () => invoke<Tag[]>("list_tags");
export const createTag = (name: string) =>
  invoke<number>("create_tag", { name });
export const renameTag = (id: number, name: string) =>
  invoke<void>("rename_tag", { id, name });
export const setTagColor = (id: number, color: string) =>
  invoke<void>("set_tag_color", { id, color });
export const deleteTag = (id: number) => invoke<void>("delete_tag", { id });
export const reorderTags = (ids: number[]) =>
  invoke<void>("reorder_tags", { ids });
export const setArticleTag = (articleId: number, tagId: number, on: boolean) =>
  invoke<void>("set_article_tag", { articleId, tagId, on });

// ── filter rules ──
export const listRules = () => invoke<Rule[]>("list_rules");
export const createRule = (
  name: string,
  feedId: number | null,
  field: RuleField,
  query: string,
  action: RuleAction,
) => invoke<number>("create_rule", { name, feedId, field, query, action });
export const updateRule = (
  id: number,
  name: string,
  enabled: boolean,
  feedId: number | null,
  field: RuleField,
  query: string,
  action: RuleAction,
) =>
  invoke<void>("update_rule", {
    id,
    name,
    enabled,
    feedId,
    field,
    query,
    action,
  });
export const deleteRule = (id: number) => invoke<void>("delete_rule", { id });
export const previewRule = (
  feedId: number | null,
  field: RuleField,
  query: string,
) => invoke<RulePreview>("preview_rule", { feedId, field, query });
