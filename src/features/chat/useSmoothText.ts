import { useEffect, useRef, useState } from "react";

/**
 * Reveal streaming text at a steady pace instead of in bursts.
 *
 * Tokens do not arrive evenly. The provider sends a handful, then a burst of
 * twenty, then pauses; painting each burst as it lands is what reads as "choppy".
 * This holds the received text in a buffer and reveals it on animation frames.
 *
 * The speed is not fixed. Each frame reveals `backlog / LAG_SECONDS`, so the
 * display rate settles at the rate text is actually arriving: a fast model shows
 * fast, a slow one shows slowly, and a one-off burst is spread over a few frames
 * rather than dumped at once. The constant lag is roughly
 * `arrival_rate * LAG_SECONDS` characters — about two dozen characters at a
 * typical streaming rate.
 *
 * The whole received string is already what the store keeps; this only decides
 * how much of it is on screen, so copies and scroll height still see the full
 * text once the turn ends.
 */
const MIN_CHARS_PER_SECOND = 60;
/** Ceiling per frame: at 60fps this is ~27 characters, still smooth to the eye. */
const MAX_CHARS_PER_SECOND = 1600;
/** How many seconds of backlog the display keeps behind the stream. */
const LAG_SECONDS = 0.075;

function reducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export function useSmoothText(text: string): string {
  // Start where the text already is: opening a chat mid-stream should not
  // replay what has already arrived.
  const [shown, setShown] = useState(text.length);
  const target = useRef(text.length);
  const cursor = useRef(text.length);
  const frame = useRef(0);
  const last = useRef(0);
  const carry = useRef(0);

  useEffect(() => {
    target.current = text.length;
    // Reduced motion, or text replaced by a shorter one (a new attempt at the
    // same part). Otherwise even the final flush of a turn is paced out, so the
    // last words do not land as one lump the moment the turn ends.
    const snap = reducedMotion() || text.length < cursor.current;
    if (snap) {
      cursor.current = text.length;
      carry.current = 0;
      setShown(text.length);
      return;
    }
    // A frame loop is already running and will pick up the new target.
    if (frame.current !== 0) return;

    last.current = performance.now();
    const tick = (now: number) => {
      frame.current = 0;
      // Clamp so returning from a background tab does not dump everything in
      // one frame; the next frame catches up quickly instead.
      const dt = Math.min(0.05, Math.max(0, (now - last.current) / 1000));
      last.current = now;
      const pending = target.current - cursor.current;
      if (pending <= 0) {
        carry.current = 0;
        return;
      }
      const rate = Math.min(
        MAX_CHARS_PER_SECOND,
        Math.max(MIN_CHARS_PER_SECOND, pending / LAG_SECONDS),
      );
      carry.current += rate * dt;
      const take = Math.min(Math.floor(carry.current), pending);
      if (take > 0) {
        carry.current -= take;
        cursor.current += take;
        setShown(cursor.current);
      }
      frame.current = requestAnimationFrame(tick);
    };
    frame.current = requestAnimationFrame(tick);
  }, [text]);

  useEffect(
    () => () => {
      if (frame.current !== 0) cancelAnimationFrame(frame.current);
    },
    [],
  );

  return shown >= text.length ? text : text.slice(0, shown);
}
