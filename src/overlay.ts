import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type OverlayState = "hidden" | "recording" | "transcribing";

const BAR_COUNT = 10;
const MIN_BAR_PX = 4;
const MAX_BAR_PX = 28;
const SMOOTH = 0.7;

const pill = document.getElementById("pill") as HTMLDivElement;
const barEls = Array.from(document.querySelectorAll<HTMLSpanElement>(".bar"));
const win = getCurrentWindow();

let state: OverlayState = "hidden";
const smoothed = new Array<number>(BAR_COUNT).fill(0);
let latestSpectrum: number[] | null = null;

async function setState(next: OverlayState) {
  if (state === next) return;
  state = next;
  pill.className = `state-${next}`;
  if (next === "hidden") {
    await win.hide();
    smoothed.fill(0);
    latestSpectrum = null;
    renderBars();
  } else {
    await win.show();
  }
}

function renderBars() {
  for (let i = 0; i < BAR_COUNT; i++) {
    const v = Math.min(1, Math.max(0, smoothed[i]));
    const h = MIN_BAR_PX + v * (MAX_BAR_PX - MIN_BAR_PX);
    barEls[i].style.height = `${h}px`;
  }
}

function tick() {
  if (state === "recording") {
    const target = latestSpectrum;
    for (let i = 0; i < BAR_COUNT; i++) {
      const t = target ? target[i] ?? 0 : 0;
      smoothed[i] = smoothed[i] * SMOOTH + t * (1 - SMOOTH);
    }
    renderBars();
  }
  requestAnimationFrame(tick);
}

listen<null>("recording-started", () => setState("recording"));
listen<number[]>("audio-level", (evt) => {
  latestSpectrum = evt.payload;
});
listen<null>("transcribing", () => setState("transcribing"));
listen<string>("transcription-complete", () => setState("hidden"));
listen<string>("error", () => setState("hidden"));
listen<null>("recording-cancelled", () => setState("hidden"));

requestAnimationFrame(tick);
