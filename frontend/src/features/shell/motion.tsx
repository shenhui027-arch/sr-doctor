// Task 63: one motion vocabulary for the app (design board 11). One ease-out
// curve, no overshoot; layout follow-ups use a spring with bounce 0. Motion's
// `reducedMotion="user"` turns transform animations off when the system asks.
import { animate, MotionConfig } from "motion/react";
import { useEffect, useRef } from "react";
import type { JSX, ReactNode, RefObject } from "react";

export const EASE = [0.22, 1, 0.36, 1] as const;
export const FAST = 0.15;
export const MEDIUM = 0.2;
export const SLOW = 0.32;
/** Layout follow-ups (nav indicator, list reflow): critically damped, no bounce. */
export const LAYOUT_SPRING = { type: "spring", bounce: 0, visualDuration: 0.22 } as const;

export function AppMotion({ children }: { children: ReactNode }): JSX.Element {
  return <MotionConfig reducedMotion="user" transition={{ duration: SLOW, ease: EASE }}>{children}</MotionConfig>;
}

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Fades and lifts a container in whenever `key` changes, without remounting
 * its children (the query list must keep its state across workspaces).
 */
export function useEntrance(ref: RefObject<HTMLElement | null>, key: string): void {
  const first = useRef(true);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    // The first paint is the app itself appearing; only later switches animate.
    if (first.current) { first.current = false; return; }
    if (prefersReducedMotion()) return;
    const controls = animate(element, { opacity: [0, 1], transform: ["translateY(8px)", "translateY(0px)"] }, { duration: SLOW, ease: EASE });
    return () => controls.stop();
  }, [ref, key]);
}
