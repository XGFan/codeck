import { create } from "zustand";

export interface TurnInfo {
  turn: number;
  nodeId: string;
  /** Prompt text, whitespace-collapsed. */
  text: string;
}

/**
 * Turn index of the main transcript, published by `TranscriptList` and read by the outline rail and `j`/`k`.
 * `locked` keeps `current` fixed after a programmatic jump until the user scrolls by hand.
 */
export const useReading = create<{ turns: TurnInfo[]; current: number; locked: boolean }>(() => ({
  turns: [],
  current: -1,
  locked: false,
}));

export function setTurns(turns: TurnInfo[]) {
  const prev = useReading.getState().turns;
  if (prev.length === turns.length && prev.every((t, i) => t.nodeId === turns[i]!.nodeId && t.text === turns[i]!.text))
    return;
  useReading.setState({ turns });
}

export function setCurrentTurn(turn: number, locked = false) {
  useReading.setState((s) => (s.current === turn && s.locked === locked ? s : { current: turn, locked }));
}

export function resetReading() {
  useReading.setState({ turns: [], current: -1, locked: false });
}
