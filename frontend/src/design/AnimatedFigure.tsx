import { animate } from "motion/react";
import { useLayoutEffect, useRef } from "react";
import type { JSX } from "react";
import { EASE, FAST } from "../features/shell/motion";

// The prefix may hold symbols and spaces but no letters: an honest state such
// as "Missing · not 0" is text, never a figure to count.
const NUMBER_PATTERN = /^([^\p{L}\d]*?)(-?\d[\d,]*(?:\.\d+)?)(.*)$/su;

export type ParsedAnimatedFigure = {
  prefix: string;
  target: number;
  decimals: number;
  grouped: boolean;
  suffix: string;
  unitKey: string;
};

function normalizeAffix(value: string): string {
  return value.replace(/\u00A0/g, " ").trim();
}

export function parseAnimatedFigure(value: string): ParsedAnimatedFigure | null {
  const match = NUMBER_PATTERN.exec(value);
  if (!match) return null;
  const [, prefix, digits, suffix] = match;
  const target = Number(digits.replace(/,/g, ""));
  if (!Number.isFinite(target)) return null;
  const decimals = digits.includes(".") ? digits.split(".")[1].length : 0;
  const grouped = digits.includes(",");
  const unitKey = `${normalizeAffix(prefix)}|${normalizeAffix(suffix)}`;
  return { prefix, target, decimals, grouped, suffix, unitKey };
}

export function shouldAnimateFigureTransition(
  previous: ParsedAnimatedFigure | null,
  next: ParsedAnimatedFigure | null,
): boolean {
  if (!previous || !next) return false;
  return previous.unitKey === next.unitKey && previous.target !== next.target;
}

function formatFigureNumber(value: number, decimals: number, grouped: boolean): string {
  const fixed = value.toFixed(decimals);
  if (!grouped) return fixed;
  const [whole, fraction] = fixed.split(".");
  return `${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}${fraction !== undefined ? `.${fraction}` : ""}`;
}

/**
 * Counts a numeric figure from its previous value ONLY when the before and after
 * units match; unit changes (e.g. `850 ms` -> `1.2 s`) or honest non-numeric
 * states switch directly without interpolating across incompatible units.
 */
export function AnimatedFigure({ value }: { value: string }): JSX.Element {
  const ref = useRef<HTMLSpanElement>(null);
  const previous = useRef<ParsedAnimatedFigure | null>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    const next = parseAnimatedFigure(value);
    const prev = previous.current;
    previous.current = next;
    if (!element) return;
    if (!next || !shouldAnimateFigureTransition(prev, next) || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
      element.textContent = value;
      return;
    }
    const controls = animate(prev!.target, next.target, {
      duration: FAST,
      ease: EASE,
      onUpdate: (latest) => {
        element.textContent = `${next.prefix}${formatFigureNumber(latest, next.decimals, next.grouped)}${next.suffix}`;
      },
      onComplete: () => {
        element.textContent = value;
      },
    });
    return () => {
      controls.stop();
      element.textContent = value;
    };
  }, [value]);

  return <span ref={ref}>{value}</span>;
}
