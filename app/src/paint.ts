// The office's painter: everything is a function of the size, the state and the time, drawn fresh
// each frame. Coordinates are points, y down, with the tab at x 0 to 26 and the drawer from 26.
import { Layout, deskFrame, tabFrame } from "./layout";

const wallTop = "rgba(43, 46, 66, 0.97)";
const wallBottom = "rgba(31, 33, 48, 0.97)";
const cubicle = "rgb(59, 64, 89)";
const cubicleEdge = "rgb(79, 84, 115)";
const windowFrame = "rgb(204, 209, 224)";
const windowSill = "rgb(230, 232, 242)";
const wood = "rgb(168, 125, 87)";
const text = "rgba(255, 255, 255, 0.93)";
const secondary = "rgba(255, 255, 255, 0.56)";
const idleGlow = "rgba(92, 232, 255, 0.9)";
const failedGlow = "rgb(255, 133, 97)";
const rounded = (size: number) => `700 ${size}px Nunito, system-ui, sans-serif`;
const system = (size: number) => `${size}px system-ui, -apple-system, sans-serif`;

export interface Scene {
  open: boolean;
  desks: number;
  crew: number;
  /** The hour, fractional, for the wall clock. */
  hour: number;
}

/** The left-rounded outline of a `w` by `h` box at (x, y), `inset` inside it, open on the right. */
function leftRounded(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, inset: number) {
  const r = Layout.corner;
  const d = inset;
  ctx.beginPath();
  ctx.moveTo(x + w, y + d);
  ctx.lineTo(x + r, y + d);
  ctx.arcTo(x + d, y + d, x + d, y + r, r - d);
  ctx.lineTo(x + d, y + h - r);
  ctx.arcTo(x + d, y + h - d, x + r, y + h - d, r - d);
  ctx.lineTo(x + w, y + h - d);
}

function roundedRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, r);
}

/** The wall: the gradient clipped to the left-rounded shape, and a faint rim inside its edge. */
function paintWall(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, rim: string) {
  ctx.save();
  leftRounded(ctx, x, y, w, h, 0);
  ctx.closePath();
  ctx.clip();
  const gradient = ctx.createLinearGradient(0, y, 0, y + h);
  gradient.addColorStop(0, wallTop);
  gradient.addColorStop(1, wallBottom);
  ctx.fillStyle = gradient;
  ctx.fillRect(x, y, w, h);
  ctx.restore();
  leftRounded(ctx, x, y, w, h, 0.5);
  ctx.strokeStyle = rim;
  ctx.lineWidth = 1;
  ctx.stroke();
}

function paintHeader(ctx: CanvasRenderingContext2D, width: number, scene: Scene) {
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillStyle = text;
  ctx.font = rounded(14);
  ctx.fillText("Office", 16, 9);
  ctx.fillStyle = secondary;
  ctx.font = system(10.5);
  ctx.fillText(scene.crew === 0 ? "No crew right now" : `${scene.crew} crew`, 16, 27);
  paintClock(ctx, width - 26, 23, scene.hour);
}

/** A wall clock telling `hour`. */
function paintClock(ctx: CanvasRenderingContext2D, cx: number, cy: number, hour: number) {
  const radius = 11;
  ctx.beginPath();
  ctx.arc(cx, cy, radius, 0, Math.PI * 2);
  ctx.fillStyle = "rgb(245, 245, 245)";
  ctx.fill();
  ctx.beginPath();
  ctx.arc(cx, cy, radius - 1, 0, Math.PI * 2);
  ctx.strokeStyle = "rgb(140, 102, 71)";
  ctx.lineWidth = 2;
  ctx.stroke();
  const minutes = (hour * 60) % 60;
  const hours = hour % 12;
  ctx.strokeStyle = "rgb(38, 38, 38)";
  ctx.lineCap = "round";
  for (const [turn, length, width] of [
    [hours / 12, 5, 1.8],
    [minutes / 60, 7.5, 1.2],
  ]) {
    const angle = turn * 2 * Math.PI;
    ctx.beginPath();
    ctx.moveTo(cx, cy);
    ctx.lineTo(cx + Math.sin(angle) * length, cy - Math.cos(angle) * length);
    ctx.lineWidth = width;
    ctx.stroke();
  }
  ctx.beginPath();
  ctx.arc(cx, cy, 1.3, 0, Math.PI * 2);
  ctx.fillStyle = failedGlow;
  ctx.fill();
}

/** An empty cubicle: the wall behind the desk and the window in it, at seat `index`. */
function paintCubicle(ctx: CanvasRenderingContext2D, index: number) {
  const desk = deskFrame(index);
  const ox = desk.x;
  const oy = Layout.header + desk.y;
  roundedRect(ctx, ox, oy + 4, desk.w, 98, 10);
  ctx.fillStyle = cubicle;
  ctx.fill();
  ctx.strokeStyle = cubicleEdge;
  ctx.lineWidth = 1;
  ctx.stroke();
  // The window over the monitor's place, its frame and sill; the sky in it comes with the painters.
  const frame = { x: ox + 9, y: oy + 11, w: 50, h: 30 };
  roundedRect(ctx, frame.x, frame.y, frame.w, frame.h, 3);
  ctx.fillStyle = windowFrame;
  ctx.fill();
  roundedRect(ctx, frame.x - 2.5, frame.y + frame.h - 2, frame.w + 5, 3.5, 1.5);
  ctx.fillStyle = windowSill;
  ctx.fill();
  roundedRect(ctx, frame.x + 2.5, frame.y + 2.5, frame.w - 5, frame.h - 5, 1.5);
  ctx.fillStyle = "rgb(96, 150, 214)";
  ctx.fill();
}

function paintFooter(ctx: CanvasRenderingContext2D, width: number, height: number) {
  const top = height - Layout.footer;
  ctx.beginPath();
  ctx.moveTo(14, top);
  ctx.lineTo(width - 14, top);
  ctx.strokeStyle = "rgba(255, 255, 255, 0.1)";
  ctx.lineWidth = 1;
  ctx.stroke();
}

/** The tab: the wall, a chevron pointing the way it goes, a little desk, and the crew count. */
function paintTab(ctx: CanvasRenderingContext2D, height: number, scene: Scene) {
  const tab = tabFrame(height);
  ctx.save();
  ctx.translate(tab.x, tab.y);
  paintWall(ctx, 0, 0, tab.w, tab.h, "rgba(255, 255, 255, 0.12)");
  const mid = tab.w / 2 + 1;
  const point = scene.open ? 2.5 : -2.5;
  ctx.beginPath();
  ctx.moveTo(mid - point, 12);
  ctx.lineTo(mid + point, 17);
  ctx.lineTo(mid - point, 22);
  ctx.strokeStyle = "rgba(255, 255, 255, 0.6)";
  ctx.lineWidth = 1.8;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.stroke();
  const desk = { x: mid, y: 38 };
  ctx.fillStyle = "rgba(255, 255, 255, 0.75)";
  ctx.fillRect(desk.x - 6, desk.y - 7, 12, 8);
  ctx.fillStyle = idleGlow;
  ctx.fillRect(desk.x - 4.5, desk.y - 5.5, 9, 5);
  ctx.fillStyle = wood;
  ctx.fillRect(desk.x - 8, desk.y + 2, 16, 2.2);
  ctx.font = rounded(11);
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  ctx.fillStyle = text;
  ctx.fillText(String(scene.crew), mid, 50);
  ctx.restore();
}

/** Paints the whole office, `height` points tall, into a context already scaled to points. */
export function paintOffice(ctx: CanvasRenderingContext2D, height: number, scene: Scene) {
  ctx.clearRect(0, 0, Layout.tab.w + Layout.width, height);
  ctx.save();
  ctx.translate(Layout.tab.w, 0);
  paintWall(ctx, 0, 0, Layout.width, height, "rgba(255, 255, 255, 0.1)");
  paintHeader(ctx, Layout.width, scene);
  for (let seat = 0; seat < scene.desks; seat += 1) paintCubicle(ctx, seat);
  paintFooter(ctx, Layout.width, height);
  ctx.restore();
  paintTab(ctx, height, scene);
}
