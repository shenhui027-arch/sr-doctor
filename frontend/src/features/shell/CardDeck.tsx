// Task 69 card contract for pages that stack many cards (the load views): the page keeps one screen,
// so its cards are shown one group at a time. A group is a top-level card or a row of cards under the
// view's root; everything else there (figures, filters, summary strips, notes) stays as it is. Each
// tab is named by the titles of the cards it shows, read from the rendered headings, so no wording is
// invented here. Hidden groups stay in the document.
import { useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useLocale } from "../../i18n/locale";
import "./card-deck.css";

const ROOTS = ".load-page, .load-history, .notify-page, .history-page";
const GROUP = ":scope > .history-card:not(.load-history-strip), :scope > .history-row";

function titleOf(card: Element): string {
  const heading = card.querySelector("h2");
  if (!heading) return "";
  const first = Array.from(heading.childNodes).find((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim());
  return (first?.textContent ?? heading.textContent ?? "").trim();
}

function groupTitle(group: Element): string {
  if (!group.classList.contains("history-row")) return titleOf(group);
  return Array.from(group.querySelectorAll(":scope > .history-card")).map(titleOf).filter(Boolean).join(" · ");
}

export function CardDeck({ children, resetKey }: { children: ReactNode; resetKey: string }): JSX.Element {
  const { text } = useLocale();
  const container = useRef<HTMLDivElement>(null);
  const [labels, setLabels] = useState<string[]>([]);
  const [index, setIndex] = useState(0);
  useEffect(() => { setIndex(0); }, [resetKey]);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const apply = () => {
      const root = element.querySelector(ROOTS);
      const groups = root ? Array.from(root.querySelectorAll(GROUP)) : [];
      const next = groups.map((group, position) => groupTitle(group) || text(`View ${position + 1}`, `视图 ${position + 1}`));
      setLabels((current) => current.length === next.length && current.every((label, position) => label === next[position]) ? current : next);
      root?.classList.toggle("card-deck-root", groups.length > 0);
      groups.forEach((group, position) => {
        const shown = groups.length < 2 || position === Math.min(index, groups.length - 1);
        group.classList.add("card-deck-group");
        if (shown) group.removeAttribute("hidden"); else group.setAttribute("hidden", "");
      });
    };
    apply();
    const observer = new MutationObserver(apply);
    observer.observe(element, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [index, text]);
  return <div className="card-deck" ref={container}>
    {labels.length > 1 ? <div aria-label={text("Cards on this view", "本视图的卡片")} className="seg-tabs card-deck-tabs" role="tablist">
      {labels.map((label, position) => <button aria-selected={position === Math.min(index, labels.length - 1)} className={position === Math.min(index, labels.length - 1) ? "active" : ""} key={`${position}:${label}`} onClick={() => setIndex(position)} role="tab" type="button">{label}</button>)}
    </div> : null}
    {children}
  </div>;
}
