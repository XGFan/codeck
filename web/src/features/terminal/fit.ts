// Sizes for the terminal window. The Pane's columns and rows are fixed (its size belongs to the person's
// ToH Client, or to ccrpc): the window fits them at 13px when it opens, and afterwards the font follows the
// window so they keep filling it.

export const BASE_FONT = 13;
const MIN_FONT = 6;
const MAX_FONT = 28;

/** A character cell per pixel of font size (measured once: width and line height at some font size, divided by it). */
export interface CellRatio {
  width: number;
  height: number;
}

/** The largest font (in half pixels) at which `cols` × `rows` cells fit in `width` × `height`. */
export function fontFor(cols: number, rows: number, width: number, height: number, cell: CellRatio): number {
  if (cols <= 0 || rows <= 0 || width <= 0 || height <= 0) return BASE_FONT;
  const fit = Math.min(width / (cols * cell.width), height / (rows * cell.height));
  return Math.min(MAX_FONT, Math.max(MIN_FONT, Math.floor(fit * 2) / 2));
}

/** The terminal area that holds `cols` × `rows` at the base font. */
export function areaFor(cols: number, rows: number, cell: CellRatio): { width: number; height: number } {
  return {
    width: Math.ceil(cols * cell.width * BASE_FONT),
    height: Math.ceil(rows * cell.height * BASE_FONT),
  };
}
