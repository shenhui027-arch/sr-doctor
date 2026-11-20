import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Fragment, useLayoutEffect, useRef } from "react";
import type { JSX, ReactNode } from "react";
import { useLocale } from "../i18n/locale";
import { Icon } from "./ui";

function isFocusableElement(element: Element | null): element is HTMLElement {
  if (!element) return false;
  if (typeof HTMLElement !== "undefined") return element instanceof HTMLElement;
  return typeof (element as HTMLElement).focus === "function";
}

/**
 * Captures the opener element in a layout effect when `open` transitions `false -> true`.
 * Preserves `returnFocusRef` across StrictMode simulated effect re-runs while open, and
 * across the `true -> false` commit so passive `onCloseAutoFocus` still sees the opener.
 */
export function syncDialogOpenLayout(
  open: boolean,
  wasOpenRef: { current: boolean },
  returnFocusRef: { current: HTMLElement | null },
  activeElement: Element | null,
): void {
  if (open && !wasOpenRef.current) {
    returnFocusRef.current = isFocusableElement(activeElement) ? activeElement : null;
  }
  wasOpenRef.current = open;
}

export function handleDialogCloseAutoFocus(
  event: { preventDefault(): void },
  returnFocusRef: { current: HTMLElement | null },
  restoreFocus: boolean,
): void {
  const target = returnFocusRef.current;
  returnFocusRef.current = null;
  if (!restoreFocus) {
    event.preventDefault();
    return;
  }
  if (target && target.isConnected) {
    event.preventDefault();
    target.focus();
  }
}

/**
 * Accessible modal dialog built on `@radix-ui/react-dialog` (spec §7.4):
 * Traps Tab/Shift+Tab focus, dismisses on Escape, and restores focus to the trigger element
 * unless `restoreFocus` is `false` (for navigating closes that move focus elsewhere).
 * Supports centered modals (`placement="center"`) and right-side drawers (`placement="right"`).
 */
export function Dialog(props: {
  open: boolean;
  onOpenChange(open: boolean): void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  tone?: "default" | "danger";
  width?: "md" | "lg";
  placement?: "center" | "right";
  restoreFocus?: boolean;
}): JSX.Element {
  const {
    open,
    onOpenChange,
    title,
    description,
    children,
    footer,
    tone = "default",
    width = "md",
    placement = "center",
    restoreFocus = true,
  } = props;
  const { text } = useLocale();
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);

  useLayoutEffect(() => {
    syncDialogOpenLayout(
      open,
      wasOpenRef,
      returnFocusRef,
      typeof document === "undefined" ? null : document.activeElement,
    );
  }, [open]);

  const cardClasses = [
    "modal-card",
    placement === "right" ? "modal-card--sheet" : "",
    width === "lg" ? "modal-card--wide" : "",
    tone === "danger" ? "modal-card--danger" : "",
  ].filter(Boolean).join(" ");

  const PortalWrapper = typeof document === "undefined" ? Fragment : DialogPrimitive.Portal;

  return (
    <DialogPrimitive.Root modal onOpenChange={onOpenChange} open={open}>
      <PortalWrapper>
        <DialogPrimitive.Overlay className="modal-backdrop" />
        <DialogPrimitive.Content
          {...(description === undefined || description === null ? { "aria-describedby": undefined } : {})}
          className={cardClasses}
          onCloseAutoFocus={(event) => {
            handleDialogCloseAutoFocus(event, returnFocusRef, restoreFocus);
          }}
          onEscapeKeyDown={(event) => {
            event.stopPropagation();
          }}
        >
          <header className="card-hd">
            <div className="card-title-group">
              <DialogPrimitive.Title className="card-title">{title}</DialogPrimitive.Title>
            </div>
            <DialogPrimitive.Close
              aria-label={text("Close dialog", "关闭弹窗")}
              className="icon-btn"
              type="button"
            >
              <Icon name="close" size={16} />
            </DialogPrimitive.Close>
          </header>
          <div className="card-bd">
            {description !== undefined && description !== null ? (
              <DialogPrimitive.Description className="modal-desc">{description}</DialogPrimitive.Description>
            ) : null}
            {children}
          </div>
          {footer !== undefined && footer !== null ? <footer className="card-ft">{footer}</footer> : null}
        </DialogPrimitive.Content>
      </PortalWrapper>
    </DialogPrimitive.Root>
  );
}
