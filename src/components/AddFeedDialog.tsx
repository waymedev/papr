import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as api from "../api";
import { useArticleActions } from "../hooks/articleActions";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { errorText } from "../lib/errors";
import Icon from "./Icon";

interface Props {
  onClose: () => void;
  onToast: (msg: string) => void;
}

/** Subscribe to a new feed — design-styled centered modal. */
export default function AddFeedDialog({ onClose, onToast }: Props) {
  const { t } = useTranslation();
  const actions = useArticleActions();
  const dialogRef = useRef<HTMLDivElement>(null);
  useFocusTrap(dialogRef);
  const [url, setUrl] = useState("");
  const [folderId, setFolderId] = useState<number | null>(null);
  const folders = useQuery({ queryKey: ["folders"], queryFn: api.listFolders });

  const add = useMutation({
    mutationFn: () => api.addFeed(url.trim(), folderId),
    onSuccess: (feed) => {
      // Adding a feed touches only the article-bearing caches — refreshing
      // unrelated ones (AI summaries, settings, storage) is wasted work.
      actions.refreshAfterBulk();
      onToast(t("addFeed.subscribed", { title: feed.title }));
      onClose();
    },
  });

  const submit = () => {
    if (url.trim() && !add.isPending) add.mutate();
  };

  // Escape closes the dialog from anywhere inside it, not just the input.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="addfeed-dialog-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="addfeed-dialog-title">{t("addFeed.title")}</h2>
        <p className="modal-hint">{t("addFeed.hint")}</p>
        <input
          className="modal-input"
          type="text"
          autoFocus
          placeholder="https://example.com"
          aria-label={t("addFeed.urlLabel")}
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        {(folders.data?.length ?? 0) > 0 && (
          <select
            className="s-select"
            style={{ width: "100%" }}
            aria-label={t("addFeed.folderLabel")}
            value={folderId ?? ""}
            onChange={(e) =>
              setFolderId(e.target.value ? Number(e.target.value) : null)
            }
          >
            <option value="">{t("addFeed.noFolder")}</option>
            {folders.data!.map((f) => (
              <option key={f.id} value={f.id}>
                {f.name}
              </option>
            ))}
          </select>
        )}
        {add.isError && <div className="modal-error">{errorText(add.error)}</div>}
        <div className="modal-actions">
          <button className="s-btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            className="s-btn primary"
            onClick={submit}
            disabled={!url.trim() || add.isPending}
          >
            <Icon name="plus" size={12} />
            {add.isPending ? t("addFeed.adding") : t("addFeed.subscribe")}
          </button>
        </div>
      </div>
    </div>
  );
}
