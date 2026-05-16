import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "../api";
import { useUi } from "../store";
import { usePlayer } from "../player";
import { useArticleActions } from "../hooks/articleActions";
import { renderMarkdown } from "../lib/markdown";
import { fullDate } from "../lib/feedMeta";
import { errorText } from "../lib/errors";
import { tagColor } from "../lib/tagColors";
import type { ArticleDetail } from "../types";
import Icon from "./Icon";
import TagPicker from "./TagPicker";

interface Props {
  onToast: (msg: string) => void;
}

function youtubeId(url: string | null): string | null {
  if (!url) return null;
  const m =
    url.match(/[?&]v=([\w-]{11})/) || url.match(/youtu\.be\/([\w-]{11})/);
  return m ? m[1] : null;
}

export default function Reader({ onToast }: Props) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const actions = useArticleActions();
  const id = useUi((s) => s.selectedArticleId);
  const useSerif = useUi((s) => s.useSerif);
  const focusMode = useUi((s) => s.focusMode);
  const setFocusMode = useUi((s) => s.setFocusMode);
  const aiOpen = useUi((s) => s.aiOpen);
  const setAiOpen = useUi((s) => s.setAiOpen);
  const markReadOnOpen = useUi((s) => s.prefs.markReadOnOpen);
  const markReadOnScroll = useUi((s) => s.prefs.markReadOnScroll);
  const showReadingTime = useUi((s) => s.prefs.showReadingTime);

  const [scrolled, setScrolled] = useState(false);
  const [showExtracted, setShowExtracted] = useState(true);
  const [tagPick, setTagPick] = useState<{ x: number; y: number } | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const playTrack = usePlayer((s) => s.play);
  const playingSrc = usePlayer((s) => (s.playing ? s.track?.src : null));

  const article = useQuery({
    queryKey: ["article", id],
    queryFn: () => api.getArticle(id as number),
    enabled: id != null,
  });
  const a: ArticleDetail | undefined = article.data;

  const readMinutes = useMemo(() => {
    const html = a?.extractedHtml || a?.contentHtml || "";
    const words = html
      .replace(/<[^>]+>/g, " ")
      .trim()
      .split(/\s+/)
      .filter(Boolean).length;
    const chars = html.replace(/<[^>]+>/g, "").length;
    // mixed CJK / latin estimate
    return Math.max(2, Math.round(Math.max(words / 220, chars / 480)));
  }, [a?.id]);

  // Reset scroll + extraction view on article change.
  useEffect(() => {
    setShowExtracted(true);
    setScrolled(false);
    setTagPick(null);
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [id]);

  // Mark as read once when an unread article is opened (if the user opted in).
  useEffect(() => {
    if (a && !a.isRead && markReadOnOpen) actions.setRead(a.id, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [a?.id]);

  const extract = useMutation({
    mutationFn: () => api.extractFulltext(a!.id),
    onSuccess: () => {
      setShowExtracted(true);
      qc.invalidateQueries({ queryKey: ["article", a!.id] });
      onToast(t("reader.fullTextExtracted"));
    },
    onError: (e) => onToast(errorText(e)),
  });

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    setScrolled(el.scrollTop > 8);
    // Mark read once the reader is scrolled to the foot of the article.
    if (
      markReadOnScroll &&
      a &&
      !a.isRead &&
      el.scrollHeight - el.scrollTop - el.clientHeight < 120
    ) {
      actions.setRead(a.id, true);
    }
  };

  const onBodyClick = (e: React.MouseEvent) => {
    const link = (e.target as HTMLElement).closest("a");
    if (link?.href) {
      e.preventDefault();
      openUrl(link.href).catch(() => {});
    }
  };

  const copyLink = () => {
    if (!a?.url) return;
    navigator.clipboard.writeText(a.url).then(() => onToast(t("reader.linkCopied")), () => {});
  };
  const share = () => {
    if (!a?.url) return;
    if (navigator.share) navigator.share({ title: a.title, url: a.url }).catch(() => {});
    else copyLink();
  };

  if (id == null || !a) {
    return (
      <div className="reader">
        <div className="reader-toolbar" data-tauri-drag-region />
        <div className="empty" style={{ flex: 1 }}>
          <div className="glyph">
            <Icon name="rss" size={22} />
          </div>
          <div>{t("reader.emptySelectArticle")}</div>
          <div style={{ fontSize: 11.5, color: "var(--muted-2)" }}>
            {t("reader.emptyHintPrefix")}{" "}
            <kbd
              style={{
                fontFamily: "var(--mono)",
                fontSize: 10,
                padding: "1px 5px",
                border: "1px solid var(--hair)",
                borderRadius: 3,
              }}
            >
              J
            </kbd>{" "}
            /{" "}
            <kbd
              style={{
                fontFamily: "var(--mono)",
                fontSize: 10,
                padding: "1px 5px",
                border: "1px solid var(--hair)",
                borderRadius: 3,
              }}
            >
              K
            </kbd>{" "}
            {t("reader.emptyHintSuffix")}
          </div>
        </div>
      </div>
    );
  }

  const hasExtracted = !!a.extractedHtml;
  const body =
    (showExtracted && a.extractedHtml ? a.extractedHtml : a.contentHtml) || "";
  const ytId = a.sourceType === "youtube" ? youtubeId(a.url) : null;

  return (
    <div className="reader">
      <div
        className={`reader-toolbar ${scrolled ? "scrolled" : ""}`}
        data-tauri-drag-region
      >
        <button
          className={`tb-btn ${a.isStarred ? "on" : ""}`}
          onClick={() => actions.setStarred(a.id, !a.isStarred)}
          title={t("reader.tbStar")}
        >
          <Icon name={a.isStarred ? "star-fill" : "star"} size={16} />
        </button>
        <button
          className={`tb-btn ${a.readLater ? "on" : ""}`}
          onClick={() => actions.setReadLater(a.id, !a.readLater)}
          title={t("reader.tbReadLater")}
        >
          <Icon name={a.readLater ? "bookmark-fill" : "bookmark"} size={16} />
        </button>
        <button
          className={`tb-btn ${a.tags.length > 0 ? "on" : ""}`}
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setTagPick((p) => (p ? null : { x: r.left, y: r.bottom + 6 }));
          }}
          title={t("reader.tbTags")}
        >
          <Icon name="tag" size={16} />
        </button>
        <button
          className={`tb-btn ${aiOpen ? "on" : ""}`}
          onClick={() => setAiOpen(!aiOpen)}
          title={t("reader.tbAiSummary")}
        >
          <Icon name={aiOpen ? "sparkle-fill" : "sparkle"} size={16} />
        </button>
        <button
          className={`tb-btn ${hasExtracted && showExtracted ? "on" : ""} ${
            extract.isPending ? "spinning" : ""
          }`}
          onClick={() =>
            hasExtracted ? setShowExtracted((v) => !v) : extract.mutate()
          }
          disabled={extract.isPending}
          title={hasExtracted ? t("reader.tbToggleFullText") : t("reader.tbExtractFullText")}
        >
          <Icon name="text" size={16} />
        </button>
        <button className="tb-btn" title={t("reader.tbCopyLink")} onClick={copyLink}>
          <Icon name="copy" size={16} />
        </button>
        <button className="tb-btn" title={t("reader.tbShare")} onClick={share}>
          <Icon name="share" size={16} />
        </button>
        <button
          className={`tb-btn ${focusMode ? "on" : ""}`}
          onClick={() => setFocusMode(!focusMode)}
          title={t("reader.tbFocusMode")}
        >
          <Icon name="focus" size={16} />
        </button>
        <div className="tb-btn spacer" />
        {a.url && (
          <button
            className="tb-btn"
            title={t("reader.tbOpenInBrowser")}
            onClick={() => openUrl(a.url!).catch(() => {})}
          >
            <Icon name="open" size={16} />
          </button>
        )}
      </div>

      <div className="reader-scroll" ref={scrollRef} onScroll={onScroll}>
        <article className="article reader-content" key={a.id}>
          <span className="article-feed">
            <Icon name="rss" size={13} />
            {a.feedTitle}
          </span>
          <h1 className="article-title">{a.title}</h1>
          <div className="article-meta">
            {a.author && <span className="author">{a.author}</span>}
            {a.author && a.publishedAt && <span>·</span>}
            {a.publishedAt && <span>{fullDate(a.publishedAt)}</span>}
            {showReadingTime && (
              <>
                <span>·</span>
                <span>{t("reader.readMinutes", { count: readMinutes })}</span>
              </>
            )}
            {extract.isPending && (
              <>
                <span>·</span>
                <span>{t("reader.extractingFullText")}</span>
              </>
            )}
          </div>

          {a.tags.length > 0 && (
            <div className="article-tags">
              {a.tags.map((tag) => (
                <button
                  key={tag.id}
                  className="article-tag"
                  style={{ "--tag-c": tagColor(tag.color) } as React.CSSProperties}
                  onClick={() =>
                    useUi.getState().select({ kind: "tag", value: tag.id }, tag.name)
                  }
                >
                  <span className="tag-dot" />
                  {tag.name}
                </button>
              ))}
            </div>
          )}

          {ytId ? (
            <iframe
              style={{ width: "100%", aspectRatio: "16 / 9" }}
              src={`https://www.youtube.com/embed/${ytId}`}
              title={a.title}
              allowFullScreen
            />
          ) : (
            a.imageUrl && <img src={a.imageUrl} alt="" />
          )}

          {a.enclosures
            .filter((e) => e.mimeType?.startsWith("audio"))
            .map((e, i) => {
              const isPlaying = playingSrc === e.url;
              return (
                <button
                  className={`episode ${isPlaying ? "playing" : ""}`}
                  key={`a${i}`}
                  onClick={() =>
                    playTrack({
                      articleId: a.id,
                      title: a.title,
                      feedTitle: a.feedTitle,
                      src: e.url,
                    })
                  }
                >
                  <span className="episode-play">
                    <Icon name={isPlaying ? "pause" : "play"} size={15} />
                  </span>
                  <span className="episode-text">
                    {isPlaying
                      ? t("reader.episodePlaying")
                      : t("reader.episodePlay")}
                  </span>
                </button>
              );
            })}
          {a.enclosures
            .filter((e) => e.mimeType?.startsWith("video"))
            .map((e, i) => (
              <div className="enclosure" key={`v${i}`}>
                <video controls src={e.url} />
              </div>
            ))}

          <div
            className="article-body"
            data-serif={useSerif}
            onClick={onBodyClick}
            dangerouslySetInnerHTML={{
              __html: body || `<p><em>${t("reader.noContent")}</em></p>`,
            }}
          />
        </article>
      </div>

      <AIDrawer
        open={aiOpen}
        article={a}
        onClose={() => setAiOpen(false)}
        onToast={onToast}
      />

      {tagPick && (
        <TagPicker
          articleId={a.id}
          attached={a.tags.map((tg) => tg.id)}
          x={tagPick.x}
          y={tagPick.y}
          onClose={() => setTagPick(null)}
          onToast={onToast}
        />
      )}
    </div>
  );
}

function AIDrawer({
  open,
  article,
  onClose,
  onToast,
}: {
  open: boolean;
  article: ArticleDetail;
  onClose: () => void;
  onToast: (m: string) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [text, setText] = useState<string | null>(article.aiSummary);
  const [busy, setBusy] = useState(false);

  // Reset to whatever the article already has when switching articles.
  useEffect(() => {
    setText(article.aiSummary);
    setBusy(false);
  }, [article.id]);

  // Generate a summary the first time the drawer opens for an article.
  useEffect(() => {
    if (!open || busy || text) return;
    let cancelled = false;
    setBusy(true);
    setText("");
    api
      .aiSummarize(article.id, (ev) => {
        if (cancelled) return;
        if (ev.type === "delta") setText((t) => (t ?? "") + ev.data);
        else if (ev.type === "error") onToast(ev.data);
      })
      .then(() => {
        if (!cancelled) qc.invalidateQueries({ queryKey: ["article", article.id] });
      })
      .catch((e) => !cancelled && onToast(errorText(e)))
      .finally(() => !cancelled && setBusy(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, article.id]);

  const loading = busy && !text;

  return (
    <div className={`ai-drawer ${open ? "open" : ""}`}>
      <div className="ai-head">
        <span className="accent-ico">
          <Icon name="sparkle-fill" size={15} />
        </span>
        <h3>{t("reader.aiSummaryTitle")}</h3>
        <button className="tb-btn close" onClick={onClose} title={t("common.close")}>
          <Icon name="x" size={14} />
        </button>
      </div>
      <div className="ai-body">
        {loading && (
          <div className="ai-loading">
            <span className="ai-dot" />
            <span className="ai-dot" />
            <span className="ai-dot" />
            <span style={{ marginLeft: 4 }}>{t("reader.aiReadingFullText")}</span>
          </div>
        )}
        {text && (
          <>
            <div
              className="ai-prose"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }}
            />
            <div
              style={{
                fontSize: 11,
                color: "var(--muted-2)",
                marginTop: 24,
                lineHeight: 1.5,
              }}
            >
              {t("reader.aiDisclaimer")}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
