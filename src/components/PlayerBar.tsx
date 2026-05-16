import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { usePlayer } from "../player";
import { useUi } from "../store";
import Icon from "./Icon";

const RATES = [0.75, 1, 1.25, 1.5, 2];

/** mm:ss for a seconds value; "–:––" while the duration is unknown. */
function clock(s: number): string {
  if (!isFinite(s) || s < 0) return "–:––";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

/**
 * Persistent audio mini-player. Mounted once at the app shell, it keeps a
 * podcast episode playing while the user browses other articles. Renders
 * nothing until a track is loaded.
 */
export default function PlayerBar() {
  const { t } = useTranslation();
  const track = usePlayer((s) => s.track);
  const playing = usePlayer((s) => s.playing);
  const rate = usePlayer((s) => s.rate);
  const setPlaying = usePlayer((s) => s.setPlaying);
  const toggle = usePlayer((s) => s.toggle);
  const setRate = usePlayer((s) => s.setRate);
  const close = usePlayer((s) => s.close);

  const audioRef = useRef<HTMLAudioElement>(null);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);

  // Load a new src and reset the timeline when the track changes.
  useEffect(() => {
    const el = audioRef.current;
    if (!el || !track) return;
    el.src = track.src;
    el.load();
    setTime(0);
    setDuration(0);
  }, [track?.src]);

  // Reflect play/pause intent onto the element.
  useEffect(() => {
    const el = audioRef.current;
    if (!el || !track) return;
    if (playing) el.play().catch(() => setPlaying(false));
    else el.pause();
  }, [playing, track?.src, setPlaying]);

  // Keep the element's speed in sync with the store.
  useEffect(() => {
    if (audioRef.current) audioRef.current.playbackRate = rate;
  }, [rate, track?.src]);

  if (!track) return null;

  const seek = (to: number) => {
    const el = audioRef.current;
    if (el && isFinite(to)) {
      el.currentTime = to;
      setTime(to);
    }
  };
  const nudge = (delta: number) =>
    seek(Math.min(duration || Infinity, Math.max(0, time + delta)));
  const cycleRate = () =>
    setRate(RATES[(RATES.indexOf(rate) + 1) % RATES.length] ?? 1);

  return (
    <div className="player-bar">
      <audio
        ref={audioRef}
        onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
        onLoadedMetadata={(e) => setDuration(e.currentTarget.duration)}
        onEnded={() => setPlaying(false)}
        preload="metadata"
      />

      <span className="player-ico">
        <Icon name="headphones" size={15} />
      </span>

      <div className="player-meta">
        <button
          className="player-title"
          onClick={() => useUi.getState().openArticle(track.articleId)}
          title={track.title}
        >
          {track.title}
        </button>
        <span className="player-feed">{track.feedTitle}</span>
      </div>

      <div className="player-controls">
        <button
          className="player-btn"
          onClick={() => nudge(-15)}
          title={t("player.back15")}
        >
          <Icon name="skip-back" size={15} />
        </button>
        <button
          className="player-btn play"
          onClick={toggle}
          title={playing ? t("player.pause") : t("player.play")}
        >
          <Icon name={playing ? "pause" : "play"} size={15} />
        </button>
        <button
          className="player-btn"
          onClick={() => nudge(30)}
          title={t("player.fwd30")}
        >
          <Icon name="skip-fwd" size={15} />
        </button>
      </div>

      <span className="player-time">{clock(time)}</span>
      <input
        className="player-scrub"
        type="range"
        min={0}
        max={duration || 0}
        step={1}
        value={Math.min(time, duration || 0)}
        onChange={(e) => seek(Number(e.target.value))}
      />
      <span className="player-time">{clock(duration)}</span>

      <button
        className="player-btn rate"
        onClick={cycleRate}
        title={t("player.speed")}
      >
        {rate}×
      </button>
      <button
        className="player-btn"
        onClick={close}
        title={t("player.close")}
      >
        <Icon name="x" size={14} />
      </button>
    </div>
  );
}
