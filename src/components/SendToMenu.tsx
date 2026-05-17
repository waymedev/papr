// "Send to…" share menu (feature F8).
//
// A small popover listing the configured share targets — Pocket, Instapaper,
// Kindle, Notion — for one article. Used both from the Reader toolbar and the
// article-list context menu. Only targets that have complete credentials
// (reported by `share_targets`) are enabled; the rest are shown disabled with
// a hint so the user knows where to configure them.

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../api";
import { errorText } from "../lib/errors";
import type { ShareTarget, ShareTargets } from "../types";
import Icon, { type IconName } from "./Icon";

interface Props {
  articleId: number;
  /** Anchor position (viewport coordinates) for the popover. */
  x: number;
  y: number;
  onClose: () => void;
  onToast: (msg: string) => void;
}

/** The four targets, in display order, with their icon + i18n key suffix. */
const TARGETS: { key: ShareTarget; icon: IconName }[] = [
  { key: "pocket", icon: "bookmark" },
  { key: "instapaper", icon: "text" },
  { key: "kindle", icon: "headphones" },
  { key: "notion", icon: "list" },
];

export default function SendToMenu({ articleId, x, y, onClose, onToast }: Props) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState({ left: x, top: y });
  const [targets, setTargets] = useState<ShareTargets | null>(null);
  const [busy, setBusy] = useState<ShareTarget | null>(null);

  // Clamp inside the viewport.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let left = x;
    let top = y;
    if (left + r.width > window.innerWidth - 8) left = window.innerWidth - r.width - 8;
    if (top + r.height > window.innerHeight - 8) top = window.innerHeight - r.height - 8;
    setPlace({ left: Math.max(8, left), top: Math.max(8, top) });
  }, [x, y]);

  // Load which targets are configured.
  useEffect(() => {
    api
      .shareTargets()
      .then(setTargets)
      .catch(() => setTargets({ pocket: false, instapaper: false, kindle: false, notion: false }));
  }, []);

  // Outside-click / Escape to dismiss.
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

  const send = async (target: ShareTarget) => {
    setBusy(target);
    try {
      await api.sendArticle(articleId, target);
      onToast(t("sendTo.sent", { target: t(`sendTo.${target}`) }));
      onClose();
    } catch (e) {
      onToast(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const anyConfigured =
    targets != null &&
    (targets.pocket || targets.instapaper || targets.kindle || targets.notion);

  return (
    <div
      ref={ref}
      className="ctx-menu hl-export-menu"
      role="menu"
      style={{ left: place.left, top: place.top }}
    >
      <div className="hl-export-head">{t("sendTo.heading")}</div>
      {targets == null ? (
        <div className="ctx-item" role="presentation" aria-disabled>
          {t("common.loading")}
        </div>
      ) : (
        TARGETS.map(({ key, icon }) => {
          const ok = targets[key];
          return (
            <button
              key={key}
              className="ctx-item"
              role="menuitem"
              disabled={!ok || busy != null}
              title={ok ? undefined : t("sendTo.notConfigured")}
              onClick={() => send(key)}
            >
              <Icon name={icon} size={13} /> {t(`sendTo.${key}`)}
              {!ok && (
                <span className="ctx-shortcut">{t("sendTo.setupHint")}</span>
              )}
            </button>
          );
        })
      )}
      {targets != null && !anyConfigured && (
        <div className="hl-export-head" style={{ borderTop: "1px solid var(--hair)" }}>
          {t("sendTo.noneConfigured")}
        </div>
      )}
    </div>
  );
}
