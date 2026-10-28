// Layout dump for pixel-parity work (v0.7): every visible leaf text element and
// every bordered/filled box, with its rectangle and the computed styles that
// decide its pixels. Evaluate in a page (CDP Runtime.evaluate or the browser
// pane) and diff the two lists to see exactly which box or glyph run moved.
(() => {
  const out = [];
  const keep = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0 || r.bottom < 0) return;
    const s = getComputedStyle(el);
    if (s.visibility === "hidden" || s.display === "none") return;
    const ownText = [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent.trim()).join(" ").trim();
    const boxed = s.borderTopWidth !== "0px" || s.backgroundColor !== "rgba(0, 0, 0, 0)";
    if (!ownText && !boxed) return;
    out.push({
      tag: el.tagName.toLowerCase(), text: ownText.slice(0, 40),
      x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height),
      font: ownText ? `${s.fontWeight} ${s.fontSize}/${s.lineHeight} ${s.fontFamily.split(",")[0]}` : undefined,
      color: ownText ? s.color : undefined,
      bg: s.backgroundColor !== "rgba(0, 0, 0, 0)" ? s.backgroundColor : undefined,
      border: s.borderTopWidth !== "0px" ? `${s.borderTopWidth} ${s.borderTopStyle} ${s.borderTopColor} r${s.borderTopLeftRadius}` : undefined,
    });
  };
  document.querySelectorAll("body *").forEach(keep);
  return out;
})();
