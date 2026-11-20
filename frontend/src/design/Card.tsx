import { useId } from "react";
import type { HTMLAttributes, JSX, ReactNode } from "react";

export type CardVariant = "default" | "kpi" | "chart" | "table" | "detail" | "list" | "form";
export type GridSpan = { xl?: number; lg?: number; md?: number; sm?: number };

function clampSpan(value: number, max: number): number {
  return Math.max(1, Math.min(max, Math.trunc(value)));
}

export function gridSpanClasses(span?: GridSpan): string {
  if (!span) return "";
  const classes: string[] = [];
  const lg = span.lg ?? span.xl;
  if (lg !== undefined) classes.push(`span-lg-${clampSpan(lg, 12)}`);
  if (span.xl !== undefined) classes.push(`span-xl-${clampSpan(span.xl, 12)}`);
  if (span.md !== undefined) classes.push(`span-md-${clampSpan(span.md, 8)}`);
  if (span.sm !== undefined) classes.push(`span-sm-${clampSpan(span.sm, 4)}`);
  return classes.join(" ");
}

function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

/**
 * Standard v1.1 Card contract (spec §4):
 * - Fixed `.card-hd` header (never scrolls)
 * - Single `.card-bd` body (scrolls internally by default)
 * - Optional fixed `.card-ft` footer
 */
export function Card(props: Omit<HTMLAttributes<HTMLElement>, "title" | "children"> & {
  title: ReactNode;
  subtitle?: ReactNode;
  titleExtra?: ReactNode;
  actions?: ReactNode;
  footer?: ReactNode;
  variant?: CardVariant;
  span?: GridSpan;
  scroll?: boolean;
  id?: string;
  "aria-label"?: string;
  className?: string;
  bodyClassName?: string;
  children: ReactNode;
}): JSX.Element {
  const {
    title,
    subtitle,
    titleExtra,
    actions,
    footer,
    variant = "default",
    span,
    scroll = true,
    id,
    "aria-label": ariaLabel,
    className,
    bodyClassName,
    children,
    ...rest
  } = props;
  const titleId = useId();

  return (
    <section
      aria-labelledby={ariaLabel ? undefined : titleId}
      {...rest}
      aria-label={ariaLabel}
      className={cx("card", `card--${variant}`, gridSpanClasses(span), className)}
      id={id}
    >
      <header className="card-hd">
        <div className="card-title-group">
          <h2 className="card-title" id={titleId}>{title}</h2>
          {subtitle !== undefined && subtitle !== null ? <p className="card-subtitle">{subtitle}</p> : null}
          {titleExtra}
        </div>
        {actions !== undefined && actions !== null ? <div className="card-hd-actions">{actions}</div> : null}
      </header>
      <div className={cx("card-bd", variant === "table" && "card-bd--flush", !scroll && "card-bd--no-scroll", bodyClassName)}>
        {children}
      </div>
      {footer !== undefined && footer !== null ? <footer className="card-ft">{footer}</footer> : null}
    </section>
  );
}

export function PageGrid(props: { children: ReactNode; className?: string; "aria-label"?: string }): JSX.Element {
  const { children, className, "aria-label": ariaLabel } = props;
  return (
    <div aria-label={ariaLabel} className={cx("page-grid", className)} role={ariaLabel ? "region" : undefined}>
      {children}
    </div>
  );
}
