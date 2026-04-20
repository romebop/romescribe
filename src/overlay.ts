import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type OverlayState = "hidden" | "recording" | "transcribing";

const BAR_COUNT = 10;
const MIN_BAR_PX = 4;
const MAX_BAR_PX = 28;
const DECAY = 0.82;

const pill = document.getElementById("pill") as HTMLDivElement;
const barEls = Array.from(document.querySelectorAll<HTMLSpanElement>(".bar"));
const win = getCurrentWindow();

let state: OverlayState = "hidden";
const bars = new Array<number>(BAR_COUNT).fill(0);
let shiftAccumulator = 0;
let latestPeak = 0;

async function setState(next: OverlayState) {
  if (state === next) return;
  state = next;
  pill.className = `state-${next}`;
  if (next === "hidden") {
    await win.hide();
    bars.fill(0);
    latestPeak = 0;
    renderBars();
  } else {
    await win.show();
  }
}

function renderBars() {
  for (let i = 0; i < BAR_COUNT; i++) {
    const v = Math.sqrt(Math.min(1, Math.max(0, bars[i])));
    const h = MIN_BAR_PX + v * (MAX_BAR_PX - MIN_BAR_PX);
    barEls[i].style.height = `${h}px`;
  }
}

function tick() {
  if (state === "recording") {
    for (let i = 0; i < BAR_COUNT; i++) bars[i] *= DECAY;

    const gained = Math.min(1, latestPeak * 2.5);
    shiftAccumulator += 1;
    if (shiftAccumulator >= 2) {
      shiftAccumulator = 0;
      for (let i = 0; i < BAR_COUNT - 1; i++) bars[i] = bars[i + 1];
      bars[BAR_COUNT - 1] = gained;
    } else {
      bars[BAR_COUNT - 1] = Math.max(bars[BAR_COUNT - 1], gained);
    }
    latestPeak = 0;
    renderBars();
  }
  requestAnimationFrame(tick);
}

listen<null>("recording-started", () => setState("recording"));
listen<number>("audio-level", (evt) => {
  latestPeak = Math.max(latestPeak, evt.payload);
});
listen<null>("transcribing", () => setState("transcribing"));
listen<string>("transcription-complete", () => setState("hidden"));
listen<string>("error", () => setState("hidden"));
listen<null>("recording-cancelled", () => setState("hidden"));

requestAnimationFrame(tick);
