// Audio player state. The mini-player survives article navigation, so the
// "now playing" track lives in its own global store — independent of the
// reading-pane selection. The actual <audio> element is owned by PlayerBar;
// this store only carries intent (which track, playing or paused, speed).

import { create } from "zustand";

export interface Track {
  /** The article the audio belongs to — lets the player deep-link back. */
  articleId: number;
  title: string;
  feedTitle: string;
  src: string;
}

interface PlayerState {
  track: Track | null;
  /** Whether playback should be running (PlayerBar drives the element). */
  playing: boolean;
  /** Playback speed multiplier, persisted across sessions. */
  rate: number;

  /** Load a track and start playing it. Re-playing the same src toggles. */
  play: (track: Track) => void;
  setPlaying: (v: boolean) => void;
  toggle: () => void;
  setRate: (r: number) => void;
  /** Stop playback and tear the player down. */
  close: () => void;
}

const RATE_KEY = "player.rate";
const initialRate = Number(localStorage.getItem(RATE_KEY)) || 1;

export const usePlayer = create<PlayerState>((set, get) => ({
  track: null,
  playing: false,
  rate: initialRate,

  play: (track) => {
    const cur = get().track;
    if (cur && cur.src === track.src) {
      set((s) => ({ playing: !s.playing }));
    } else {
      set({ track, playing: true });
    }
  },
  setPlaying: (playing) => set({ playing }),
  toggle: () => set((s) => ({ playing: s.track ? !s.playing : false })),
  setRate: (rate) => {
    localStorage.setItem(RATE_KEY, String(rate));
    set({ rate });
  },
  close: () => set({ track: null, playing: false }),
}));
