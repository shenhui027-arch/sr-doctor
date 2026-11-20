import * as Tabs from "@radix-ui/react-tabs";
import { motion } from "motion/react";
import { useId, useRef } from "react";
import type { JSX, ReactNode } from "react";
import { EASE, FAST } from "../features/shell/motion";

export type SegItem<V extends string> = {
  value: V;
  label: ReactNode;
  count?: ReactNode;
  disabled?: boolean;
};

const PILL_TRANSITION = { duration: FAST, ease: EASE } as const;

export function resolveNextSegTabValue<V extends string>(
  items: readonly SegItem<V>[],
  current: V,
  key: string,
): V | null {
  const enabled = items.filter((item) => !item.disabled);
  if (enabled.length === 0) return null;
  const index = enabled.findIndex((item) => item.value === current);
  if (key === "Home") return enabled[0].value;
  if (key === "End") return enabled[enabled.length - 1].value;
  if (key === "ArrowRight") {
    const nextIndex = index < 0 ? 0 : (index + 1) % enabled.length;
    return enabled[nextIndex].value;
  }
  if (key === "ArrowLeft") {
    const prevIndex = index <= 0 ? enabled.length - 1 : index - 1;
    return enabled[prevIndex].value;
  }
  return null;
}

/**
 * The product's single segmented control (spec §5.2), built on `@radix-ui/react-tabs`.
 * Active pill slides with `motion` `layoutId` (transform only, no layout reflow).
 */
export function SegTabs<V extends string>(props: {
  // V comes from `value` alone, so a typed state setter fits `onValueChange`.
  value: V;
  onValueChange(value: NoInfer<V>): void;
  items: readonly SegItem<NoInfer<V>>[];
  size?: "md" | "sm";
  "aria-label": string;
  className?: string;
  children?: ReactNode;
}): JSX.Element {
  const { value, onValueChange, items, size = "md", "aria-label": ariaLabel, className, children } = props;
  const pillId = `seg-pill-${useId()}`;
  const pointerDownTabRef = useRef<V | null>(null);
  const latestValueRef = useRef(value);
  latestValueRef.current = value;
  // Radix TabsTrigger (automatic mode) calls onValueChange synchronously inside both
  // focus() and onMouseDown; deduplicate within the same turn while still allowing
  // subsequent switches if a parent ignores a transition.
  const pendingValueRef = useRef<V | null>(null);
  pendingValueRef.current = null;
  const emitValueChange = (next: V) => {
    const effective = pendingValueRef.current ?? latestValueRef.current;
    if (next === effective) return;
    pendingValueRef.current = next;
    queueMicrotask(() => {
      pendingValueRef.current = null;
    });
    onValueChange(next);
  };
  const listClassName = ["seg-tabs", "has-pill", size === "sm" ? "seg-tabs--sm" : "", className ?? ""].filter(Boolean).join(" ");

  const list = (
    <Tabs.List aria-label={ariaLabel} className={listClassName} loop>
      {items.map((item) => {
        const active = item.value === value;
        return (
          <Tabs.Trigger
            className={active ? "seg-tab is-active" : "seg-tab"}
            disabled={item.disabled}
            key={item.value}
            onMouseDown={() => {
              if (!item.disabled) pointerDownTabRef.current = item.value;
            }}
            onClick={(event) => {
              // Radix handles pointer clicks via onMouseDown; only forward synthetic .click() (detail === 0).
              const fromPointer = pointerDownTabRef.current === item.value;
              pointerDownTabRef.current = null;
              if (!fromPointer && event.detail === 0 && !item.disabled && item.value !== value) {
                emitValueChange(item.value);
              }
            }}
            value={item.value}
          >
            {active ? <motion.span aria-hidden="true" className="seg-tab-pill" layoutId={pillId} transition={PILL_TRANSITION} /> : null}
            <span className="seg-tab-content">
              <span>{item.label}</span>
              {item.count !== undefined && item.count !== null ? <span className="seg-count">{item.count}</span> : null}
            </span>
          </Tabs.Trigger>
        );
      })}
    </Tabs.List>
  );

  if (children !== undefined && children !== null) {
    return (
      <Tabs.Root activationMode="automatic" onValueChange={(next) => emitValueChange(next as V)} value={value}>
        {list}
        {children}
      </Tabs.Root>
    );
  }

  return (
    <Tabs.Root activationMode="automatic" asChild onValueChange={(next) => emitValueChange(next as V)} value={value}>
      {list}
    </Tabs.Root>
  );
}
