// Shared motion vocabulary (v1.1 spec §5.7): one ease-out curve, no bounce,
// transform and opacity only, 160 / 200 / 240ms durations.
import { animate, MotionConfig } from "motion/react";
import { useEffect, useRef } from "react";
import type { JSX, ReactNode, RefObject } from "react";

export const EASE = [0.22, 1, 0.36, 1] as const;
export const FAST = 0.16;
export const MEDIUM = 0.2;
export const SLOW = 0.24;
/** Layout transitions (sliding pills, surface indicators): single easing curve, zero bounce. */
export const LAYOUT_SPRING = { duration: MEDIUM, ease: EASE } as const;

export function AppMotion({ children }: { children: ReactNode }): JSX.Element {
  return <MotionConfig reducedMotion="user" transition={{ duration: MEDIUM, ease: EASE }}>{children}</MotionConfig>;
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
    if (first.current) { first.current = false; return; }
    if (prefersReducedMotion()) return;
    const controls = animate(element, { opacity: [0, 1], transform: ["translateY(6px)", "translateY(0px)"] }, { duration: MEDIUM, ease: EASE });
    return () => controls.stop();
  }, [ref, key]);
}
