import * as Tooltip from "@radix-ui/react-tooltip";
import { createContext, useContext } from "react";
import type { JSX, ReactNode } from "react";

const PATHS = {
  info: <><circle cx="12" cy="12" r="9" /><path d="M12 16v-4" /><path d="M12 8h.01" /></>,
  settings: <><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l-.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" /><circle cx="12" cy="12" r="3" /></>,
  upload: <><path d="M12 15V3" /><path d="m7 8 5-5 5 5" /><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /></>,
  chevronRight: <path d="m9 18 6-6-6-6" />,
  chevronDown: <path d="m6 9 6 6 6-6" />,
  back: <path d="m15 18-6-6 6-6" />,
  close: <><path d="M18 6 6 18" /><path d="m6 6 12 12" /></>,
  play: <path d="M6 4v16l14-8z" />,
  search: <><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></>,
  download: <><path d="M12 3v12" /><path d="m7 10 5 5 5-5" /><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /></>,
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 16 }: { name: IconName; size?: number }): JSX.Element {
  return <svg aria-hidden="true" className="ui-icon" fill="none" height={size} stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" viewBox="0 0 24 24" width={size}>{PATHS[name]}</svg>;
}

const TooltipRootMountedContext = createContext(false);

/** Mounted once at the app root; nested calls are no-ops so one shared delay state governs the app. */
export function TooltipProvider({ children }: { children: ReactNode }): JSX.Element {
  const alreadyMounted = useContext(TooltipRootMountedContext);
  if (alreadyMounted) return <>{children}</>;
  return (
    <TooltipRootMountedContext.Provider value={true}>
      <Tooltip.Provider delayDuration={200} skipDelayDuration={300}>
        {children}
      </Tooltip.Provider>
    </TooltipRootMountedContext.Provider>
  );
}

/**
 * A tooltip on any trigger. Uses the single root TooltipProvider in the app
 * (falling back to a local provider only in isolated component unit tests).
 */
export function Tip({ content, children, side = "top" }: { content: ReactNode; children: JSX.Element; side?: "top" | "bottom" | "left" | "right" }): JSX.Element {
  const hasRootProvider = useContext(TooltipRootMountedContext);
  const node = (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="ui-tip" collisionPadding={12} side={side} sideOffset={8}>{content}</Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
  return hasRootProvider ? node : <TooltipProvider>{node}</TooltipProvider>;
}

/** 24x24px accessible info button (WCAG 2.2 AA target size) replacing always-on helper text. */
export function InfoTip({ label, children, side }: { label: string; children: ReactNode; side?: "top" | "bottom" | "left" | "right" }): JSX.Element {
  return (
    <Tip content={children} side={side}>
      <button aria-label={label} className="info-btn ui-info" type="button"><Icon name="info" size={14} /></button>
    </Tip>
  );
}

export type EvidenceLevel = "complete" | "partial" | "sparse" | "missing";

const FILLED: Record<EvidenceLevel, number> = { complete: 3, partial: 2, sparse: 1, missing: 0 };

/**
 * Three-segment evidence completeness meter. `label` is the localized statement
 * used as both the accessible name and tooltip lead.
 */
export function EvidenceMeter({ level, label, detail }: { level: EvidenceLevel; label: string; detail?: ReactNode }): JSX.Element {
  const filled = FILLED[level];
  return (
    <Tip content={detail ? <>{label}<br />{detail}</> : label}>
      <span aria-label={label} className="evidence-meter ui-meter" data-level={level} role="img" tabIndex={0}>
        <span aria-hidden="true" className="evidence-bars">
          {[0, 1, 2].map((index) => <i data-on={index < filled ? "" : undefined} key={index} />)}
        </span>
      </span>
    </Tip>
  );
}
