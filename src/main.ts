import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// -- Elements --
const pill = document.getElementById("voice-ring")!;
const statusText = document.getElementById("status-text")!;
const versionEl = document.getElementById("version")!;
const providerEl = document.getElementById("provider") as HTMLSelectElement;
const modelEl = document.getElementById("model-name") as HTMLSelectElement;
const geminiKeyEl = document.getElementById("gemini-api-key") as HTMLInputElement;
const qwenKeyEl = document.getElementById("qwen-api-key") as HTMLInputElement;
const geminiKeyHint = document.getElementById("gemini-key-hint")!;
const qwenKeyHint = document.getElementById("qwen-key-hint")!;
const promptEditor = document.getElementById("prompt-editor") as HTMLTextAreaElement;
const hotkeyEl = document.getElementById("hotkey") as HTMLInputElement;
const audioDeviceEl = document.getElementById("audio-device") as HTMLSelectElement;
const settingsForm = document.getElementById("settings-form") as HTMLFormElement;
const saveStatus = document.getElementById("save-status")!;
const settingsPanel = document.getElementById("settings-panel")!;
const promptsPanel = document.getElementById("prompts-panel")!;
const toggleSettings = document.getElementById("toggle-settings")!;
const togglePrompts = document.getElementById("toggle-prompts")!;
const promptStatus = document.getElementById("prompt-status")!;

// -- Types --
interface Config {
  provider: string;
  gemini_api_key: string;
  qwen_api_key: string;
  model_name: string;
  hotkey: string;
  has_gemini_key: boolean;
  has_qwen_key: boolean;
}

interface Prompts {
  agent: string;
  rules: string;
  vocabulary: string;
}

interface StatusEvent {
  status: string;
  detail: string | null;
}

// -- Prompt editor state --
let currentPrompts: Prompts = { agent: "", rules: "", vocabulary: "" };
let activeTab: keyof Prompts = "agent";

// -- Panel toggles (mutual exclusive) --
function openPanel(panel: HTMLElement, btn: HTMLElement) {
  const other = panel === settingsPanel ? promptsPanel : settingsPanel;
  const otherBtn = panel === settingsPanel ? togglePrompts : toggleSettings;
  other.classList.remove("open");
  otherBtn.classList.remove("active");
  panel.classList.toggle("open");
  btn.classList.toggle("active");
}

toggleSettings.addEventListener("click", () => openPanel(settingsPanel, toggleSettings));
togglePrompts.addEventListener("click", () => openPanel(promptsPanel, togglePrompts));

// -- Status --
let errorTimer: ReturnType<typeof setTimeout> | null = null;

function updateStatus(status: string, detail?: string | null) {
  pill.className = "rec-pill";
  statusText.style.color = "";

  switch (status) {
    case "Recording":
      pill.classList.add("recording");
      statusText.textContent = "Listening…";
      break;
    case "Recognizing":
      pill.classList.add("recognizing");
      statusText.textContent = "Recognizing…";
      break;
    case "Done":
      pill.classList.add("done");
      statusText.textContent = "Done";
      break;
    case "Error":
      pill.classList.add("error");
      statusText.textContent = detail || "Error";
      statusText.style.color = "var(--pill-rec)";
      if (errorTimer) clearTimeout(errorTimer);
      errorTimer = setTimeout(() => updateStatus("Ready"), 4000);
      break;
    default:
      statusText.textContent = "Tap to start";
      break;
  }
}

// -- Model sync --
function syncModels() {
  const g = document.getElementById("gemini-models") as HTMLOptGroupElement;
  const q = document.getElementById("qwen-models") as HTMLOptGroupElement;
  const isGemini = providerEl.value === "gemini";

  if (isGemini) {
    g.style.display = ""; q.style.display = "none";
    if (modelEl.value.startsWith("qwen")) modelEl.value = "gemini-3-flash-preview";
  } else {
    g.style.display = "none"; q.style.display = "";
    if (modelEl.value.startsWith("gemini")) modelEl.value = "qwen3.5-omni-flash";
  }

  document.getElementById("gemini-key-field")!.style.display = isGemini ? "" : "none";
  document.getElementById("qwen-key-field")!.style.display = isGemini ? "none" : "";
}

// -- Prompt tabs --
function switchTab(tab: keyof Prompts) {
  // Save current editor to state
  currentPrompts[activeTab] = promptEditor.value;
  activeTab = tab;
  promptEditor.value = currentPrompts[tab];

  document.querySelectorAll(".tab").forEach((t) => {
    t.classList.toggle("active", (t as HTMLElement).dataset.tab === tab);
  });
}

document.querySelectorAll(".tab").forEach((t) => {
  t.addEventListener("click", () => {
    switchTab((t as HTMLElement).dataset.tab as keyof Prompts);
  });
});

// -- Init --
async function init() {
  const version: string = await invoke("get_version");
  versionEl.textContent = `v${version}`;

  // Config
  const config: Config = await invoke("get_config");
  providerEl.value = config.provider;
  modelEl.value = config.model_name;
  hotkeyEl.value = config.hotkey;
  hotkeyEl.dataset.prev = config.hotkey;
  // Show current hotkey in the hint below the pill
  const hintEl = document.getElementById("hotkey-hint");
  if (hintEl) hintEl.textContent = config.hotkey.replace(/\+/g, " + ");

  if (config.has_gemini_key) {
    geminiKeyEl.placeholder = "••••••••";
    geminiKeyHint.textContent = "Configured";
    geminiKeyHint.style.color = "var(--green)";
  } else {
    geminiKeyHint.textContent = "Not set";
    geminiKeyHint.style.color = "var(--text3)";
  }

  if (config.has_qwen_key) {
    qwenKeyEl.placeholder = "••••••••";
    qwenKeyHint.textContent = "Configured";
    qwenKeyHint.style.color = "var(--green)";
  } else {
    qwenKeyHint.textContent = "Not set";
    qwenKeyHint.style.color = "var(--text3)";
  }

  syncModels();

  // Prompts
  currentPrompts = await invoke("get_prompts");
  promptEditor.value = currentPrompts[activeTab];

  // Auto-open settings if current provider has no key
  const needsKey =
    (config.provider === "gemini" && !config.has_gemini_key) ||
    (config.provider === "qwen" && !config.has_qwen_key);
  if (needsKey) {
    settingsPanel.classList.add("open");
    toggleSettings.classList.add("active");
  }

  // Devices
  const devices: string[] = await invoke("list_audio_devices");
  audioDeviceEl.innerHTML = "";
  if (devices.length === 0) {
    audioDeviceEl.innerHTML = '<option value="">No device</option>';
  } else {
    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d;
      opt.textContent = d;
      audioDeviceEl.appendChild(opt);
    }
  }

  await listen<StatusEvent>("notype://status", (e) => {
    updateStatus(e.payload.status, e.payload.detail);
  });
}

// -- Hotkey capture --
// Prevent manual typing — only accept keyboard shortcut recording
hotkeyEl.addEventListener("beforeinput", (e) => e.preventDefault());
hotkeyEl.addEventListener("paste", (e) => e.preventDefault());
hotkeyEl.addEventListener("focus", () => { hotkeyEl.value = "Press shortcut…"; });

hotkeyEl.addEventListener("keydown", (e) => {
  e.preventDefault();
  e.stopPropagation();

  // Ignore lone modifier keys — wait for the actual key
  if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

  // Must have at least one modifier
  if (!e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey) return;

  const parts: string[] = [];
  if (e.metaKey) parts.push("Cmd");      // macOS Command key
  if (e.ctrlKey) parts.push("Ctrl");     // Control key
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");

  // Use e.code for reliable physical key name (e.key can be garbled with modifiers)
  const key = codeToKeyName(e.code);
  if (!key) return;
  parts.push(key);

  hotkeyEl.value = parts.join("+");
  hotkeyEl.blur();
});

hotkeyEl.addEventListener("blur", () => {
  if (hotkeyEl.value === "Press shortcut…") {
    hotkeyEl.value = hotkeyEl.dataset.prev || "Ctrl+.";
  } else {
    hotkeyEl.dataset.prev = hotkeyEl.value;
  }
});

/** Map KeyboardEvent.code to a clean key name for display + backend parsing */
function codeToKeyName(code: string): string | null {
  // Letters: KeyA → A
  if (code.startsWith("Key")) return code.slice(3);
  // Digits: Digit0 → 0
  if (code.startsWith("Digit")) return code.slice(5);
  // Common keys
  const map: Record<string, string> = {
    Period: ".", Comma: ",", Slash: "/", Backslash: "\\",
    Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]",
    Minus: "-", Equal: "=", Backquote: "`",
    Space: "Space", Enter: "Enter", Tab: "Tab",
    Escape: "Escape", Backspace: "Backspace", Delete: "Delete",
    ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
    F1: "F1", F2: "F2", F3: "F3", F4: "F4", F5: "F5", F6: "F6",
    F7: "F7", F8: "F8", F9: "F9", F10: "F10", F11: "F11", F12: "F12",
  };
  return map[code] || null;
}

// -- Events --
providerEl.addEventListener("change", syncModels);

document.querySelectorAll(".toggle-key-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    const input = document.getElementById((btn as HTMLElement).dataset.target!) as HTMLInputElement;
    input.type = input.type === "password" ? "text" : "password";
  });
});

// Save prompts
document.getElementById("save-prompt")!.addEventListener("click", async () => {
  currentPrompts[activeTab] = promptEditor.value;
  promptStatus.textContent = "Saving…";
  promptStatus.style.color = "var(--text3)";
  try {
    await invoke("save_prompts", { dto: currentPrompts });
    promptStatus.textContent = "Saved";
    promptStatus.style.color = "var(--green)";
    setTimeout(() => { promptStatus.textContent = ""; }, 2000);
  } catch (err) {
    promptStatus.textContent = `${err}`;
    promptStatus.style.color = "var(--pill-rec)";
  }
});

// Reset prompt to builtin
document.getElementById("reset-prompt")!.addEventListener("click", async () => {
  const builtins: Prompts = await invoke("get_builtin_prompts");
  currentPrompts[activeTab] = builtins[activeTab];
  promptEditor.value = builtins[activeTab];
  promptStatus.textContent = "Reset to default";
  promptStatus.style.color = "var(--text2)";
  setTimeout(() => { promptStatus.textContent = ""; }, 2000);
});

// Save settings
settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  saveStatus.textContent = "Saving…";
  saveStatus.style.color = "var(--text3)";

  try {
    await invoke("save_config", {
      dto: {
        provider: providerEl.value,
        gemini_api_key: geminiKeyEl.value,
        qwen_api_key: qwenKeyEl.value,
        model_name: modelEl.value,
        hotkey: hotkeyEl.value,
        has_gemini_key: true,
        has_qwen_key: true,
      },
    });

    saveStatus.textContent = "Saved";
    saveStatus.style.color = "var(--green)";

    if (geminiKeyEl.value) {
      geminiKeyEl.value = "";
      geminiKeyEl.placeholder = "••••••••";
      geminiKeyHint.textContent = "Configured";
      geminiKeyHint.style.color = "var(--green)";
    }
    if (qwenKeyEl.value) {
      qwenKeyEl.value = "";
      qwenKeyEl.placeholder = "••••••••";
      qwenKeyHint.textContent = "Configured";
      qwenKeyHint.style.color = "var(--green)";
    }

    setTimeout(() => { saveStatus.textContent = ""; }, 2000);
  } catch (err) {
    saveStatus.textContent = `${err}`;
    saveStatus.style.color = "var(--pill-rec)";
  }
});

window.addEventListener("DOMContentLoaded", init);
