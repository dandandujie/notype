import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// -- Elements --
const statusDot = document.getElementById("status-dot")!;
const statusGlow = document.getElementById("status-glow")!;
const statusText = document.getElementById("status-text")!;
const versionEl = document.getElementById("version")!;
const previewText = document.getElementById("preview-text")!;
const previewBox = document.getElementById("preview-box")!;

const providerEl = document.getElementById("provider") as HTMLSelectElement;
const modelEl = document.getElementById("model-name") as HTMLSelectElement;
const apiKeyEl = document.getElementById("api-key") as HTMLInputElement;
const toggleKeyBtn = document.getElementById("toggle-key")!;
const keyHint = document.getElementById("key-hint")!;
const hotkeyEl = document.getElementById("hotkey") as HTMLInputElement;
const audioDeviceEl = document.getElementById("audio-device") as HTMLSelectElement;
const settingsForm = document.getElementById("settings-form") as HTMLFormElement;
const saveStatus = document.getElementById("save-status")!;

// -- Types --
interface Config {
  provider: string;
  api_key: string;
  model_name: string;
  hotkey: string;
  has_api_key: boolean;
}

interface StatusEvent {
  status: string;
  detail: string | null;
}

// -- Status Colors (CSS custom property values) --
const STATUS_STYLES: Record<string, { color: string; css: string }> = {
  Ready:       { color: "var(--status-ready)",       css: "#3dd68c" },
  Recording:   { color: "var(--status-recording)",   css: "#f05656" },
  Recognizing: { color: "var(--status-recognizing)", css: "#f0a030" },
  Done:        { color: "var(--status-done)",         css: "#5b9cf5" },
  Error:       { color: "var(--status-error)",        css: "#f05656" },
};

let errorRecoveryTimer: ReturnType<typeof setTimeout> | null = null;

function updateStatus(status: string, detail?: string | null) {
  const style = STATUS_STYLES[status] || STATUS_STYLES.Ready;

  statusDot.style.background = style.color;
  statusDot.style.boxShadow = `0 0 12px ${style.css}`;
  statusGlow.style.background = style.color;

  statusText.textContent = status;

  if (status === "Recording") {
    statusDot.classList.add("pulse");
    statusGlow.style.opacity = "0.3";
  } else {
    statusDot.classList.remove("pulse");
    statusGlow.style.opacity = "0.15";
  }

  if (status === "Done" && detail) {
    previewText.textContent = detail;
    previewBox.classList.add("visible");
  }

  if (status === "Error" && detail) {
    previewText.textContent = detail;
    previewBox.classList.add("visible");
    if (errorRecoveryTimer) clearTimeout(errorRecoveryTimer);
    errorRecoveryTimer = setTimeout(() => updateStatus("Ready"), 5000);
  }
}

// -- Provider/Model sync --
function syncModelOptions() {
  const provider = providerEl.value;
  const geminiGroup = document.getElementById("gemini-models") as HTMLOptGroupElement;
  const qwenGroup = document.getElementById("qwen-models") as HTMLOptGroupElement;

  if (provider === "gemini") {
    geminiGroup.style.display = "";
    qwenGroup.style.display = "none";
    if (modelEl.value.startsWith("qwen")) {
      modelEl.value = "gemini-3-flash";
    }
  } else {
    geminiGroup.style.display = "none";
    qwenGroup.style.display = "";
    if (modelEl.value.startsWith("gemini")) {
      modelEl.value = "qwen3.5-omni-flash";
    }
  }
}

// -- Init --
async function init() {
  const version: string = await invoke("get_version");
  versionEl.textContent = `v${version}`;

  const config: Config = await invoke("get_config");
  providerEl.value = config.provider;
  modelEl.value = config.model_name;
  hotkeyEl.value = config.hotkey;

  const banner = document.getElementById("setup-banner")!;
  if (config.has_api_key) {
    apiKeyEl.placeholder = "••••••••  (configured)";
    keyHint.textContent = "Leave empty to keep current key";
    keyHint.style.color = "var(--status-ready)";
    banner.style.display = "none";
  } else {
    keyHint.textContent = "Required — voice input is disabled";
    keyHint.style.color = "var(--status-error)";
    banner.style.display = "flex";
  }

  syncModelOptions();

  const devices: string[] = await invoke("list_audio_devices");
  audioDeviceEl.innerHTML = "";
  if (devices.length === 0) {
    audioDeviceEl.innerHTML = '<option value="">No devices found</option>';
  } else {
    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d;
      opt.textContent = d;
      audioDeviceEl.appendChild(opt);
    }
  }

  await listen<StatusEvent>("notype://status", (event) => {
    updateStatus(event.payload.status, event.payload.detail);
  });
}

// -- Events --
providerEl.addEventListener("change", syncModelOptions);

toggleKeyBtn.addEventListener("click", () => {
  apiKeyEl.type = apiKeyEl.type === "password" ? "text" : "password";
});

settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();

  const btn = document.getElementById("save-btn")!;
  btn.classList.add("saving");
  saveStatus.textContent = "Saving...";
  saveStatus.style.color = "var(--text-tertiary)";

  try {
    await invoke("save_config", {
      dto: {
        provider: providerEl.value,
        api_key: apiKeyEl.value,
        model_name: modelEl.value,
        hotkey: hotkeyEl.value,
        has_api_key: true,
      },
    });

    saveStatus.textContent = "Saved";
    saveStatus.style.color = "var(--status-ready)";
    keyHint.textContent = "Key configured";
    keyHint.style.color = "var(--status-ready)";
    apiKeyEl.value = "";
    apiKeyEl.placeholder = "••••••••  (configured)";

    const banner = document.getElementById("setup-banner")!;
    banner.style.display = "none";

    setTimeout(() => {
      saveStatus.textContent = "";
      btn.classList.remove("saving");
    }, 2000);
  } catch (err) {
    saveStatus.textContent = `${err}`;
    saveStatus.style.color = "var(--status-error)";
    btn.classList.remove("saving");
  }
});

// -- Boot --
window.addEventListener("DOMContentLoaded", init);
