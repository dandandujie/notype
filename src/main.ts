import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { prepareWithSegments, layoutWithLines } from "@chenglou/pretext";

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
const doubaoKeyEl = document.getElementById("doubao-api-key") as HTMLInputElement;
const doubaoKeyHint = document.getElementById("doubao-key-hint")!;
const doubaoBaseUrlEl = document.getElementById("doubao-base-url") as HTMLInputElement;
const doubaoOfficialAppKeyEl = document.getElementById("doubao-official-app-key") as HTMLInputElement;
const doubaoOfficialAppKeyHint = document.getElementById("doubao-official-app-key-hint")!;
const doubaoOfficialAccessKeyEl = document.getElementById("doubao-official-access-key") as HTMLInputElement;
const doubaoOfficialAccessKeyHint = document.getElementById("doubao-official-access-key-hint")!;
const doubaoPostprocessEnabledEl = document.getElementById("doubao-postprocess-enabled") as HTMLSelectElement;
const doubaoPostprocessProviderEl = document.getElementById("doubao-postprocess-provider") as HTMLSelectElement;
const doubaoRealtimeWsEnabledEl = document.getElementById("doubao-realtime-ws-enabled") as HTMLSelectElement;
const doubaoImeCredentialPathEl = document.getElementById("doubao-ime-credential-path") as HTMLInputElement;
const doubaoSetupBtn = document.getElementById("doubao-setup-btn") as HTMLButtonElement;
const doubaoSetupStatus = document.getElementById("doubao-setup-status")!;
const doubaoPostprocessHint = document.getElementById("doubao-postprocess-hint")!;
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
  doubao_api_key: string;
  doubao_base_url: string;
  doubao_official_app_key: string;
  doubao_official_access_key: string;
  enable_doubao_postprocess: boolean;
  doubao_postprocess_provider: string;
  enable_doubao_realtime_ws: boolean;
  doubao_ime_credential_path: string;
  model_name: string;
  hotkey: string;
  has_gemini_key: boolean;
  has_qwen_key: boolean;
  has_doubao_key: boolean;
  has_doubao_official_app_key: boolean;
  has_doubao_official_access_key: boolean;
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

interface DoubaoRealtimeSetupResult {
  python: string;
  credential_path: string;
  base_url: string;
  model_name: string;
  installed: boolean;
  credential_ready: boolean;
  gateway_running: boolean;
  message: string;
}

let hasGeminiKey = false;
let hasQwenKey = false;

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
function isDoubaoOfficialModel(model: string) {
  const v = model.trim().toLowerCase();
  return (
    v === "doubao-asr-official" ||
    v === "doubao-asr-official-standard" ||
    v === "doubao-asr-official-flash" ||
    v === "official" ||
    v === "official-standard" ||
    v === "official-flash" ||
    v === "standard" ||
    v === "flash"
  );
}

function syncModels() {
  const g = document.getElementById("gemini-models") as HTMLOptGroupElement;
  const q = document.getElementById("qwen-models") as HTMLOptGroupElement;
  const d = document.getElementById("doubao-models") as HTMLOptGroupElement;
  const isGemini = providerEl.value === "gemini";
  const isQwen = providerEl.value === "qwen";
  const isDoubao = providerEl.value === "doubao";

  g.style.display = isGemini ? "" : "none";
  q.style.display = isQwen ? "" : "none";
  d.style.display = isDoubao ? "" : "none";

  const invalidModel =
    (isGemini && !modelEl.value.startsWith("gemini")) ||
    (isQwen && !modelEl.value.startsWith("qwen")) ||
    (isDoubao && !modelEl.value.startsWith("doubao-asr"));

  if (invalidModel) {
    if (isGemini) modelEl.value = "gemini-3-flash-preview";
    else if (isQwen) modelEl.value = "qwen3.5-omni-flash";
    else modelEl.value = "doubao-asr";
  }

  document.getElementById("gemini-key-field")!.style.display = isGemini ? "" : "none";
  document.getElementById("qwen-key-field")!.style.display = isQwen ? "" : "none";
  document.getElementById("doubao-fields")!.style.display = isDoubao ? "" : "none";
  document.getElementById("doubao-official-key-fields")!.style.display =
    isDoubao && isDoubaoOfficialModel(modelEl.value) ? "" : "none";
}

function updateDoubaoPostprocessHint() {
  const enabled = doubaoPostprocessEnabledEl.value === "true";
  const preferred = doubaoPostprocessProviderEl.value;
  const realtimeWs = doubaoRealtimeWsEnabledEl.value === "true";
  const preferredLabel =
    preferred === "qwen" ? "Qwen" : preferred === "gemini" ? "Gemini" : "Auto(Qwen→Gemini)";

  const wsLabel = realtimeWs ? "WS实时预览已开启" : "WS实时预览已关闭";

  if (!enabled) {
    doubaoPostprocessHint.textContent = `${wsLabel}；已关闭 LLM 后处理：仅输出 ASR 粗转写`;
    doubaoPostprocessHint.style.color = "var(--text3)";
    return;
  }

  if (hasQwenKey || hasGeminiKey) {
    doubaoPostprocessHint.textContent = `${wsLabel}；实时后处理已启用（偏好 ${preferredLabel}）`;
    doubaoPostprocessHint.style.color = "var(--green)";
  } else {
    doubaoPostprocessHint.textContent = `${wsLabel}；已启用但未配置 Qwen/Gemini Key：将退化为 ASR 粗转写`;
    doubaoPostprocessHint.style.color = "var(--text3)";
  }
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
  doubaoBaseUrlEl.value = config.doubao_base_url || "http://127.0.0.1:8000";
  doubaoPostprocessEnabledEl.value = config.enable_doubao_postprocess ? "true" : "false";
  doubaoPostprocessProviderEl.value = config.doubao_postprocess_provider || "auto";
  doubaoRealtimeWsEnabledEl.value = config.enable_doubao_realtime_ws ? "true" : "false";
  doubaoImeCredentialPathEl.value =
    config.doubao_ime_credential_path || "~/.config/doubaoime-asr/credentials.json";
  hotkeyEl.value = config.hotkey;
  hotkeyEl.dataset.prev = config.hotkey;
  // Show current hotkey in the hint below the pill
  const hintEl = document.getElementById("hotkey-hint");
  if (hintEl) hintEl.textContent = config.hotkey.replace(/\+/g, " + ");

  hasGeminiKey = config.has_gemini_key;
  hasQwenKey = config.has_qwen_key;

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

  if (config.has_doubao_key) {
    doubaoKeyEl.placeholder = "••••••••";
    doubaoKeyHint.textContent = "Configured";
    doubaoKeyHint.style.color = "var(--green)";
  } else {
    doubaoKeyHint.textContent = "Optional";
    doubaoKeyHint.style.color = "var(--text3)";
  }

  if (config.has_doubao_official_app_key) {
    doubaoOfficialAppKeyEl.placeholder = "••••••••";
    doubaoOfficialAppKeyHint.textContent = "Configured";
    doubaoOfficialAppKeyHint.style.color = "var(--green)";
  } else {
    doubaoOfficialAppKeyHint.textContent = "Not set";
    doubaoOfficialAppKeyHint.style.color = "var(--text3)";
  }

  if (config.has_doubao_official_access_key) {
    doubaoOfficialAccessKeyEl.placeholder = "••••••••";
    doubaoOfficialAccessKeyHint.textContent = "Configured";
    doubaoOfficialAccessKeyHint.style.color = "var(--green)";
  } else {
    doubaoOfficialAccessKeyHint.textContent = "Not set";
    doubaoOfficialAccessKeyHint.style.color = "var(--text3)";
  }

  updateDoubaoPostprocessHint();

  syncModels();

  // Prompts
  currentPrompts = await invoke("get_prompts");
  promptEditor.value = currentPrompts[activeTab];

  // Auto-open settings if current provider has no key
  const needsKey =
    (config.provider === "gemini" && !config.has_gemini_key) ||
    (config.provider === "qwen" && !config.has_qwen_key) ||
    (config.provider === "doubao" &&
      isDoubaoOfficialModel(config.model_name) &&
      (!config.has_doubao_official_app_key || !config.has_doubao_official_access_key));
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
modelEl.addEventListener("change", syncModels);
doubaoPostprocessEnabledEl.addEventListener("change", updateDoubaoPostprocessHint);
doubaoPostprocessProviderEl.addEventListener("change", updateDoubaoPostprocessHint);
doubaoRealtimeWsEnabledEl.addEventListener("change", updateDoubaoPostprocessHint);

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
        doubao_api_key: doubaoKeyEl.value,
        doubao_base_url: doubaoBaseUrlEl.value,
        doubao_official_app_key: doubaoOfficialAppKeyEl.value,
        doubao_official_access_key: doubaoOfficialAccessKeyEl.value,
        enable_doubao_postprocess: doubaoPostprocessEnabledEl.value === "true",
        doubao_postprocess_provider: doubaoPostprocessProviderEl.value,
        enable_doubao_realtime_ws: doubaoRealtimeWsEnabledEl.value === "true",
        doubao_ime_credential_path: doubaoImeCredentialPathEl.value,
        model_name: modelEl.value,
        hotkey: hotkeyEl.value,
        has_gemini_key: true,
        has_qwen_key: true,
        has_doubao_key: true,
        has_doubao_official_app_key: true,
        has_doubao_official_access_key: true,
      },
    });

    saveStatus.textContent = "Saved";
    saveStatus.style.color = "var(--green)";

    if (geminiKeyEl.value) {
      geminiKeyEl.value = "";
      geminiKeyEl.placeholder = "••••••••";
      geminiKeyHint.textContent = "Configured";
      geminiKeyHint.style.color = "var(--green)";
      hasGeminiKey = true;
    }
    if (qwenKeyEl.value) {
      qwenKeyEl.value = "";
      qwenKeyEl.placeholder = "••••••••";
      qwenKeyHint.textContent = "Configured";
      qwenKeyHint.style.color = "var(--green)";
      hasQwenKey = true;
    }
    if (doubaoKeyEl.value) {
      doubaoKeyEl.value = "";
      doubaoKeyEl.placeholder = "••••••••";
      doubaoKeyHint.textContent = "Configured";
      doubaoKeyHint.style.color = "var(--green)";
    }
    if (doubaoOfficialAppKeyEl.value) {
      doubaoOfficialAppKeyEl.value = "";
      doubaoOfficialAppKeyEl.placeholder = "••••••••";
      doubaoOfficialAppKeyHint.textContent = "Configured";
      doubaoOfficialAppKeyHint.style.color = "var(--green)";
    }
    if (doubaoOfficialAccessKeyEl.value) {
      doubaoOfficialAccessKeyEl.value = "";
      doubaoOfficialAccessKeyEl.placeholder = "••••••••";
      doubaoOfficialAccessKeyHint.textContent = "Configured";
      doubaoOfficialAccessKeyHint.style.color = "var(--green)";
    }

    updateDoubaoPostprocessHint();

    setTimeout(() => { saveStatus.textContent = ""; }, 2000);
  } catch (err) {
    saveStatus.textContent = `${err}`;
    saveStatus.style.color = "var(--pill-rec)";
  }
});

// One-click setup for doubao realtime runtime + credential bootstrap
doubaoSetupBtn.addEventListener("click", async () => {
  doubaoSetupBtn.disabled = true;
  doubaoSetupStatus.textContent = "Installing runtime, bootstrapping credentials, and starting local gateway…";
  doubaoSetupStatus.style.color = "var(--text3)";

  try {
    const desiredPath = doubaoImeCredentialPathEl.value.trim();
    const desiredBaseUrl = doubaoBaseUrlEl.value.trim();
    const desiredGatewayApiKey = doubaoKeyEl.value.trim();
    const result = await invoke<DoubaoRealtimeSetupResult>("setup_doubao_realtime_runtime", {
      credentialPath: desiredPath || null,
      baseUrl: desiredBaseUrl || null,
      gatewayApiKey: desiredGatewayApiKey || null,
    });
    doubaoImeCredentialPathEl.value = result.credential_path;
    doubaoBaseUrlEl.value = result.base_url;
    providerEl.value = "doubao";
    modelEl.value = result.model_name || "doubao-asr";
    syncModels();
    if (desiredGatewayApiKey) {
      doubaoKeyEl.value = "";
      doubaoKeyEl.placeholder = "••••••••";
      doubaoKeyHint.textContent = "Configured";
      doubaoKeyHint.style.color = "var(--green)";
    }
    saveStatus.textContent = "Saved";
    saveStatus.style.color = "var(--green)";
    setTimeout(() => { saveStatus.textContent = ""; }, 2000);
    doubaoSetupStatus.textContent = `Ready (${result.python}): ${result.message}`;
    doubaoSetupStatus.style.color = "var(--green)";
  } catch (err) {
    doubaoSetupStatus.textContent = `${err}`;
    doubaoSetupStatus.style.color = "var(--pill-rec)";
  } finally {
    doubaoSetupBtn.disabled = false;
  }
});

// -- Prompt Preview (Pretext Canvas) --
const previewWrap = document.getElementById("prompt-preview-wrap")!;
const previewCanvas = document.getElementById("prompt-preview-canvas") as HTMLCanvasElement;
const togglePreviewBtn = document.getElementById("toggle-preview")!;
let previewVisible = false;

const PREVIEW_FONT_SIZE = 11;
const PREVIEW_FONT = `${PREVIEW_FONT_SIZE}px -apple-system, BlinkMacSystemFont, SF Pro Text, Helvetica Neue, sans-serif`;
const PREVIEW_LINE_HEIGHT = 16;
const PREVIEW_DPR = window.devicePixelRatio || 2;

function renderPromptPreview() {
  if (!previewVisible) return;

  // Compose full prompt from current state
  currentPrompts[activeTab] = promptEditor.value;
  const parts = [currentPrompts.agent, currentPrompts.rules, currentPrompts.vocabulary]
    .filter((s) => s.trim().length > 0);
  const composed = parts.join("\n\n");

  if (!composed) {
    previewCanvas.style.display = "none";
    return;
  }
  previewCanvas.style.display = "";

  const padH = 16;
  const padV = 12;
  const cssW = previewCanvas.parentElement!.clientWidth - 36; // account for wrap padding
  const textW = cssW - padH * 2;

  const prepared = prepareWithSegments(composed, PREVIEW_FONT, { whiteSpace: "pre-wrap" });
  const result = layoutWithLines(prepared, textW, PREVIEW_LINE_HEIGHT);

  const cssH = result.height + padV * 2;
  previewCanvas.style.width = `${cssW}px`;
  previewCanvas.style.height = `${cssH}px`;
  previewCanvas.width = Math.round(cssW * PREVIEW_DPR);
  previewCanvas.height = Math.round(cssH * PREVIEW_DPR);

  const ctx = previewCanvas.getContext("2d")!;
  ctx.scale(PREVIEW_DPR, PREVIEW_DPR);

  // Background
  ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim() || "#f2f0ed";
  ctx.beginPath();
  ctx.roundRect(0, 0, cssW, cssH, 8);
  ctx.fill();

  // Section colors for visual distinction
  const sectionColors = ["#1d1d1f", "#5856d6", "#34a853"];
  let sectionIdx = 0;
  ctx.font = PREVIEW_FONT;
  ctx.textBaseline = "alphabetic";

  // Map lines to sections by tracking "\n\n" boundaries in the composed text
  const sectionStarts: number[] = [0];
  let searchFrom = 0;
  for (let i = 1; i < parts.length; i++) {
    const sep = composed.indexOf("\n\n", searchFrom + parts[i - 1].length - 1);
    if (sep >= 0) {
      sectionStarts.push(sep + 2);
      searchFrom = sep + 2;
    }
  }

  let charOffset = 0;
  for (let li = 0; li < result.lines.length; li++) {
    // Determine which section this line belongs to
    const lineStart = charOffset;
    while (sectionIdx < sectionStarts.length - 1 && lineStart >= sectionStarts[sectionIdx + 1]) {
      sectionIdx++;
    }
    ctx.fillStyle = sectionColors[sectionIdx % sectionColors.length];

    const y = padV + li * PREVIEW_LINE_HEIGHT + PREVIEW_FONT_SIZE;
    ctx.fillText(result.lines[li].text, padH, y);
    charOffset += result.lines[li].text.length;
    // Account for newline character
    if (li < result.lines.length - 1) charOffset++;
  }
}

togglePreviewBtn.addEventListener("click", () => {
  previewVisible = !previewVisible;
  previewWrap.classList.toggle("hidden", !previewVisible);
  togglePreviewBtn.style.color = previewVisible ? "var(--text)" : "";
  if (previewVisible) renderPromptPreview();
});

// Re-render preview on prompt edit (debounced)
let previewTimer: ReturnType<typeof setTimeout> | null = null;
promptEditor.addEventListener("input", () => {
  if (!previewVisible) return;
  if (previewTimer) clearTimeout(previewTimer);
  previewTimer = setTimeout(renderPromptPreview, 300);
});

window.addEventListener("DOMContentLoaded", init);
