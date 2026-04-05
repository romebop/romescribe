import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
// --- Elements ---
const modelList = document.getElementById("model-list")!;
const gpuToggle = document.getElementById("gpu-toggle")! as HTMLInputElement;
const hotkeyInput = document.getElementById("hotkey-input")! as HTMLInputElement;
const hotkeyCheck = document.getElementById("hotkey-check")!;

// --- Types ---
interface ModelEntry {
  id: string;
  name: string;
  description: string;
  size_bytes: number;
  downloaded: boolean;
  active: boolean;
}

interface Settings {
  selected_model: string;
  use_gpu: boolean;
  hotkey: string;
}

interface DownloadProgress {
  model_id: string;
  downloaded_bytes: number;
  total_bytes: number;
}

// --- State ---
let downloadingModel: string | null = null;
let currentHotkey = "";

// --- Helpers ---
function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return (bytes / 1_000_000_000).toFixed(1) + " GB";
  if (bytes >= 1_000_000) return (bytes / 1_000_000).toFixed(0) + " MB";
  return (bytes / 1_000).toFixed(0) + " KB";
}

// --- Settings ---
async function loadSettings() {
  const [settings, models] = await Promise.all([
    invoke<Settings>("get_settings"),
    invoke<ModelEntry[]>("get_models"),
  ]);

  gpuToggle.checked = settings.use_gpu;
  currentHotkey = settings.hotkey;
  hotkeyInput.value = settings.hotkey;
  renderModelList(models);
}

function renderModelList(models: ModelEntry[]) {
  modelList.innerHTML = "";

  for (const model of models) {
    const row = document.createElement("div");
    row.classList.add("model-row");
    if (model.active) row.classList.add("active");
    row.id = `model-${model.id}`;

    const isDownloading = downloadingModel === model.id;

    let actionHtml: string;
    if (model.active) {
      actionHtml = `<button class="btn-active" disabled>Active</button>`;
    } else if (isDownloading) {
      actionHtml = `<button class="btn-cancel" data-action="cancel" data-model="${model.id}">Cancel</button>`;
    } else if (model.downloaded) {
      actionHtml = `<button class="btn-select" data-action="select" data-model="${model.id}">Select</button>`;
    } else {
      actionHtml = `<button class="btn-download" data-action="download" data-model="${model.id}">Download</button>`;
    }

    row.innerHTML = `
      <div class="model-info">
        <div class="model-name">${model.name}</div>
        <div class="model-meta">${formatBytes(model.size_bytes)} &middot; ${model.description}</div>
        ${isDownloading ? `
          <div class="progress-container">
            <div class="progress-bar"><div class="progress-fill" id="progress-${model.id}"></div></div>
            <div class="progress-text" id="progress-text-${model.id}">Starting download...</div>
          </div>
        ` : ""}
      </div>
      <div class="model-action">${actionHtml}</div>
    `;

    modelList.appendChild(row);
  }
}

// --- Model Actions ---
modelList.addEventListener("click", async (e) => {
  const btn = (e.target as HTMLElement).closest("button");
  if (!btn) return;

  const action = btn.getAttribute("data-action");
  const modelId = btn.getAttribute("data-model");
  if (!action || !modelId) return;

  if (action === "download") {
    try {
      downloadingModel = modelId;
      await invoke("download_model", { modelId });
      loadSettings();
    } catch (err) {
      downloadingModel = null;
      loadSettings();
      alert(`Download failed: ${err}`);
    }
  } else if (action === "select") {
    btn.textContent = "Loading...";
    btn.setAttribute("disabled", "true");
    try {
      await invoke("select_model", { modelId });
      loadSettings();
    } catch (err) {
      alert(`Failed to select model: ${err}`);
      loadSettings();
    }
  } else if (action === "cancel") {
    await invoke("cancel_download");
  }
});

// --- GPU Toggle ---
gpuToggle.addEventListener("change", async () => {
  const useGpu = gpuToggle.checked;
  gpuToggle.disabled = true;
  try {
    await invoke("set_use_gpu", { useGpu });
  } catch (err) {
    gpuToggle.checked = !useGpu;
    alert(`Failed to change GPU setting: ${err}`);
  }
  gpuToggle.disabled = false;
});

// --- Hotkey ---
hotkeyInput.addEventListener("keydown", async (e) => {
  e.preventDefault();
  const modifiers: string[] = [];
  if (e.metaKey) modifiers.push("Cmd");
  if (e.ctrlKey) modifiers.push("Ctrl");
  if (e.altKey) modifiers.push("Option");
  if (e.shiftKey) modifiers.push("Shift");

  // Use e.code for the physical key (immune to IME/Option key character mapping)
  const code = e.code;
  // Only modifier keys pressed — wait for the actual key
  if (["MetaLeft", "MetaRight", "ControlLeft", "ControlRight",
       "AltLeft", "AltRight", "ShiftLeft", "ShiftRight"].includes(code)) return;
  // Need at least one modifier + a key
  if (modifiers.length === 0) return;

  // Map e.code to our key name format
  let keyName: string;
  if (code === "Space") keyName = "Space";
  else if (code === "Enter") keyName = "Enter";
  else if (code === "Escape") keyName = "Escape";
  else if (code === "Tab") keyName = "Tab";
  else if (code.startsWith("Key")) keyName = code.slice(3); // "KeyR" -> "R"
  else if (code.startsWith("Digit")) keyName = code.slice(5); // "Digit1" -> "1"
  else if (code.startsWith("F") && /^F\d+$/.test(code)) keyName = code; // "F1" etc
  else keyName = code;

  const newHotkey = [...modifiers, keyName].join("+");
  hotkeyInput.value = newHotkey;

  if (newHotkey === currentHotkey) return;

  try {
    await invoke("set_hotkey", { hotkey: newHotkey });
    currentHotkey = newHotkey;
    hotkeyCheck.textContent = "\u2713";
    hotkeyCheck.classList.add("visible");
    setTimeout(() => { hotkeyCheck.classList.remove("visible"); }, 1200);
  } catch (err) {
    hotkeyInput.value = currentHotkey;
    hotkeyCheck.textContent = "\u2717";
    hotkeyCheck.classList.add("visible");
    setTimeout(() => { hotkeyCheck.classList.remove("visible"); }, 1200);
  }
});

// --- Events ---
listen<DownloadProgress>("download-progress", (event) => {
  const { model_id, downloaded_bytes, total_bytes } = event.payload;
  const fill = document.getElementById(`progress-${model_id}`);
  const text = document.getElementById(`progress-text-${model_id}`);
  if (fill) {
    const pct = total_bytes > 0 ? (downloaded_bytes / total_bytes) * 100 : 0;
    fill.style.width = `${pct}%`;
  }
  if (text) {
    text.textContent = `${formatBytes(downloaded_bytes)} / ${formatBytes(total_bytes)}`;
  }
});

listen<string>("download-complete", () => {
  downloadingModel = null;
  loadSettings();
});

listen<string>("download-error", (event) => {
  downloadingModel = null;
  loadSettings();
  console.error("Download error:", event.payload);
});

listen<string>("navigate", () => {
  loadSettings();
});

// --- Init ---
loadSettings();
