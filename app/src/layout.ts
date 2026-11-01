// The office's measurements in points, the same numbers as the Rust `layout` module and the Swift
// app's `OfficeLayout`, so the three agree on where everything is.
export const Layout = {
  width: 300,
  tab: { w: 26, h: 76 },
  header: 46,
  footer: 52,
  inset: 12,
  gap: 10,
  desk: { w: 133, h: 158 },
  corner: 14,
  /** The window's extra width left of the tab, for the shadow. */
  margin: 12,
} as const;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Desk `index`'s frame inside the desk area, which starts under the header. */
export function deskFrame(index: number): Rect {
  const { inset, gap, desk } = Layout;
  return {
    x: inset + (index % 2) * (desk.w + gap),
    y: Math.floor(index / 2) * (desk.h + gap),
    w: desk.w,
    h: desk.h,
  };
}

/** The tab's frame in the sliding container, whose x 0 is the tab's left edge. */
export function tabFrame(height: number): Rect {
  return { x: 0, y: Math.round((height - Layout.tab.h) / 2), w: Layout.tab.w, h: Layout.tab.h };
}

export function contains(r: Rect, x: number, y: number): boolean {
  return x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
}
