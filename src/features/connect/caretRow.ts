// Where the caret is drawn in a textarea, measured on a hidden copy laid out the same way.

const COPIED = [
  "fontFamily",
  "fontSize",
  "fontWeight",
  "fontStyle",
  "letterSpacing",
  "lineHeight",
  "wordBreak",
  "overflowWrap",
  "tabSize",
  "textIndent",
] as const;

/** The caret (no selection) is on the textarea's first visual row: ↑ there has no row to go up to. */
export function caretOnFirstRow(el: HTMLTextAreaElement): boolean {
  const pos = el.selectionStart;
  if (pos !== el.selectionEnd) return false;
  const before = el.value.slice(0, pos);
  if (before.includes("\n")) return false;
  if (pos === 0) return true;
  const cs = getComputedStyle(el);
  const copy = document.createElement("div");
  for (const p of COPIED) copy.style[p] = cs[p];
  Object.assign(copy.style, {
    position: "absolute",
    visibility: "hidden",
    top: "0",
    left: "0",
    whiteSpace: "pre-wrap",
    boxSizing: "content-box",
    padding: "0",
    width: `${el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight)}px`,
  });
  copy.textContent = before;
  const mark = document.createElement("span");
  mark.textContent = "​";
  copy.append(mark);
  document.body.append(copy);
  const first = mark.offsetTop < (parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.5) / 2;
  copy.remove();
  return first;
}
