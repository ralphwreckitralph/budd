// The office page: a canvas in a sliding container. Rust owns the state (open or shut, the desks)
// and the pointer; this page draws and reports clicks on the tab.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Layout, contains, tabFrame } from "./layout";
import { paintOffice, type Scene } from "./paint";

interface OfficeState {
  open: boolean;
  desks: number;
}

interface Pointer {
  /** In drawer coordinates: the tab spans x -26 to 0. */
  x: number;
  y: number;
}

const office = document.getElementById("office") as HTMLDivElement;
const tabShadow = document.getElementById("tab-shadow") as HTMLDivElement;
const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const ctx = canvas.getContext("2d");
if (!ctx) throw new Error("no 2d context");

let state: OfficeState = { open: false, desks: 2 };
let pointer: Pointer | null = null;
let height = window.innerHeight;

/** The hour, fractional, by the Mac's clock. */
function hourNow(): number {
  const now = new Date();
  return now.getHours() + now.getMinutes() / 60 + now.getSeconds() / 3600;
}

/** Sizes the canvas to the window at the device's pixel ratio. */
function fit() {
  height = window.innerHeight;
  const width = Layout.tab.w + Layout.width;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * dpr);
  canvas.height = Math.round(height * dpr);
  ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
  tabShadow.style.top = `${tabFrame(height).y}px`;
  painted = "";
}

let painted = "";

function scene(): Scene {
  return { open: state.open, desks: state.desks, crew: 0, hour: hourNow() };
}

/** What a frame depends on: the state, the clock's minute, the size and the pointer. */
function sceneKey(s: Scene): string {
  return `${s.open}|${s.desks}|${Math.floor(s.hour * 60)}|${height}|${pointer?.x},${pointer?.y}`;
}

/** 12 ticks a second shut, 15 open. A tick paints only when something visible changed, and asks
 * for an animation frame only then, so an idle office costs the page no rendering work at all.
 * Nothing in the shell animates yet; the painters that do will mark every tick dirty. */
function frame() {
  const now = scene();
  const key = sceneKey(now);
  if (key !== painted) {
    painted = key;
    requestAnimationFrame(() => paintOffice(ctx!, height, now));
  }
  setTimeout(frame, 1000 / (state.open ? 15 : 12));
}

function apply(next: OfficeState) {
  const changed = next.open !== state.open;
  state = next;
  if (changed) {
    office.classList.add("sliding");
    office.classList.toggle("open", state.open);
    office.setAttribute("aria-label", `Budd's office, ${state.open ? "open" : "shut"}`);
  }
  painted = "";
}

office.addEventListener("transitionend", () => office.classList.remove("sliding"));

canvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) return;
  if (contains(tabFrame(height), event.offsetX, event.offsetY)) {
    void invoke<OfficeState>("office_toggle").then(apply);
  }
});

window.addEventListener("resize", fit);

async function start() {
  fit();
  await Promise.all([
    document.fonts.load("700 14px Nunito").catch(() => undefined),
    listen<OfficeState>("office", (event) => apply(event.payload)),
    listen<Pointer | null>("pointer", (event) => {
      pointer = event.payload;
    }),
  ]);
  await listen<string>("snapshot", (event) => {
    // Dev only: the canvas as a PNG, saved by Rust under the folder given.
    const png = canvas.toDataURL("image/png").split(",")[1];
    void invoke("snapshot_save", { dir: event.payload, name: "office", png });
  });
  apply(await invoke<OfficeState>("office_state"));
  frame();
}

void start();

/** The pointer in drawer coordinates, for the painters that come next. */
export function currentPointer(): Pointer | null {
  return pointer;
}
