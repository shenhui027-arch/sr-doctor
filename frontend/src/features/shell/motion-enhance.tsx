// Task 70 (board 11, user: "增加 motion 使用，让整个页面更灵动"): motion that every page shares.
// - Segmented controls: the current option's surface slides to the chosen one instead of jumping.
// - Figures: a changed number counts to its new value.
// One ease-out curve, no overshoot, transform/opacity only (the indicator also tracks the option's
// width); everything stays still when the system asks for reduced motion.
import { animate } from "motion/react";
import { useEffect, useLayoutEffect, useRef } from "react";
import type { JSX } from "react";
import { EASE } from "./motion";
import "./motion.css";

const GROUPS = ".seg-tabs, .audit-view-switch, .alert-category, .history-tabs, .audit-overview-range, .locale-switch";
const ACTIVE = ':scope > button.active, :scope > button[aria-selected="true"], :scope > button[aria-pressed="true"], :scope > button[aria-checked="true"]';

function place(group: HTMLElement): void {
  let indicator = group.querySelector<HTMLElement>(":scope > .seg-indicator");
  if (!indicator) {
    indicator = document.createElement("span");
    indicator.className = "seg-indicator";
    indicator.setAttribute("aria-hidden", "true");
    group.prepend(indicator);
    group.classList.add("has-indicator");
  }
  const active = group.querySelector<HTMLElement>(ACTIVE);
  if (!active || active.offsetWidth === 0) { indicator.style.opacity = "0"; return; }
  // The first placement lands without travelling; later moves slide.
  const first = indicator.dataset.placed !== "1";
  if (first) indicator.style.transition = "none";
  indicator.style.opacity = "1";
  indicator.style.width = `${active.offsetWidth}px`;
  indicator.style.height = `${active.offsetHeight}px`;
  indicator.style.transform = `translate(${active.offsetLeft}px, ${active.offsetTop}px)`;
  if (first) { indicator.dataset.placed = "1"; void indicator.offsetWidth; indicator.style.transition = ""; }
}

/** Mounted once by the app: keeps a sliding surface under the chosen option of every segmented control. */
export function useSegmentIndicators(): void {
  useEffect(() => {
    let frame = 0;
    const run = () => { frame = 0; document.querySelectorAll<HTMLElement>(GROUPS).forEach(place); };
    const schedule = () => { if (!frame) frame = window.requestAnimationFrame(run); };
    run();
    const observer = new MutationObserver(schedule);
    observer.observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ["class", "aria-selected", "aria-pressed", "aria-checked", "hidden"] });
    window.addEventListener("resize", schedule);
    return () => { observer.disconnect(); window.removeEventListener("resize", schedule); if (frame) window.cancelAnimationFrame(frame); };
  }, []);
}

const NUMBER = /^(\D*?)(-?\d[\d,]*(?:\.\d+)?)(.*)$/s;

function format(value: number, decimals: number, grouped: boolean): string {
  const fixed = value.toFixed(decimals);
  if (!grouped) return fixed;
  const [whole, fraction] = fixed.split(".");
  return `${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}${fraction !== undefined ? `.${fraction}` : ""}`;
}

/**
 * A figure that counts to its value when it appears or changes. The text is
 * always the backend's display text at rest: only its leading number moves,
 * with the same decimals and grouping, and a text without a number is shown
 * as it is (missing values are never turned into 0).
 */
export function AnimatedFigure({ value }: { value: string }): JSX.Element {
  const ref = useRef<HTMLSpanElement>(null);
  const previous = useRef<number | null>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    const match = NUMBER.exec(value);
    if (!element || !match) return;
    const [, prefix, digits, suffix] = match;
    const target = Number(digits.replace(/,/g, ""));
    const decimals = digits.includes(".") ? digits.split(".")[1].length : 0;
    const grouped = digits.includes(",");
    const from = previous.current ?? 0;
    previous.current = target;
    if (!Number.isFinite(target) || from === target || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    const controls = animate(from, target, {
      duration: 0.6, ease: EASE,
      onUpdate: (latest) => { element.textContent = `${prefix}${format(latest, decimals, grouped)}${suffix}`; },
      onComplete: () => { element.textContent = value; },
    });
    return () => { controls.stop(); element.textContent = value; };
  }, [value]);
  return <span ref={ref}>{value}</span>;
}
