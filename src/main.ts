import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { prepareWithSegments, layoutWithLines } from "@chenglou/pretext";

// ============ Types ============

interface Config {
  provider: string;
  gemini_api_key: string;
  qwen_api_key: string;
  mimo_api_key: string;
  mimo_base_url: string;
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
  audio_device: string;
  input_mode: string;
  auto_copy: boolean;
  has_gemini_key: boolean;
  has_qwen_key: boolean;
  has_mimo_key: boolean;
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

interface HistoryEntry {
  id: number;
  text: string;
  provider: string;
  model: string;
  duration_secs: number;
}

interface AudioDevice {
  name: string;
  is_default: boolean;
}

interface SaveResult {
  restart_needed: boolean;
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

// ============ Element refs ============

const $ = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id) as T;

const brandLed = $("brand-led");
const statusText = $("status-text");
const recTimer = $("rec-timer");
const recBtn = $("rec-btn");
const recCancel = $("rec-cancel");
const resultCard = $("result-card");
const resultLabel = $("result-label");
const resultTime = $("result-time");
const resultText = $("result-text");
const copyResultBtn = $("copy-result");

const historyList = $("history-list");
const historyEmpty = $("history-empty");
const clearHistoryBtn = $("clear-history");

const promptEditor = $<HTMLTextAreaElement>("prompt-editor");
const promptStatus = $("prompt-status");

const providerSeg = $("provider-seg");
const modelEl = $<HTMLSelectElement>("model-name");
const geminiKeyEl = $<HTMLInputElement>("gemini-api-key");
const qwenKeyEl = $<HTMLInputElement>("qwen-api-key");
const mimoKeyEl = $<HTMLInputElement>("mimo-api-key");
const mimoBaseUrlEl = $<HTMLInputElement>("mimo-base-url");
const geminiKeyHint = $("gemini-key-hint");
const qwenKeyHint = $("qwen-key-hint");
const mimoKeyHint = $("mimo-key-hint");
const doubaoKeyEl = $<HTMLInputElement>("doubao-api-key");
const doubaoKeyHint = $("doubao-key-hint");
const doubaoBaseUrlEl = $<HTMLInputElement>("doubao-base-url");
const doubaoOfficialAppKeyEl = $<HTMLInputElement>("doubao-official-app-key");
const doubaoOfficialAppKeyHint = $("doubao-official-app-key-hint");
const doubaoOfficialAccessKeyEl = $<HTMLInputElement>("doubao-official-access-key");
const doubaoOfficialAccessKeyHint = $("doubao-official-access-key-hint");
const doubaoPostprocessEnabledEl = $<HTMLInputElement>("doubao-postprocess-enabled");
const doubaoPostprocessProviderEl = $<HTMLSelectElement>("doubao-postprocess-provider");
const doubaoRealtimeWsEnabledEl = $<HTMLInputElement>("doubao-realtime-ws-enabled");
const doubaoImeCredentialPathEl = $<HTMLInputElement>("doubao-ime-credential-path");
const doubaoSetupBtn = $<HTMLButtonElement>("doubao-setup-btn");
const doubaoSetupStatus = $("doubao-setup-status");
const doubaoPostprocessHint = $("doubao-postprocess-hint");

const hotkeyEl = $<HTMLInputElement>("hotkey");
const audioDeviceEl = $<HTMLSelectElement>("audio-device");
const inputModeEl = $<HTMLSelectElement>("input-mode");
const autoCopyEl = $<HTMLInputElement>("auto-copy");
const autostartEl = $<HTMLInputElement>("autostart");
const settingsForm = $<HTMLFormElement>("settings-form");
const saveStatus = $("save-status");
const versionEl = $("version");

// ============ State ============

let hasGeminiKey = false;
let hasQwenKey = false;
let currentProvider = "gemini";
let currentPrompts: Prompts = { agent: "", rules: "", vocabulary: "" };
let activeTab: keyof Prompts = "agent";
let isRecording = false;
let historyCache: HistoryEntry[] = [];
let historyLoaded = false;

// ============ View router ============

const navButtons = Array.from(
  document.querySelectorAll<HTMLButtonElement>(".nav-btn")
);

function showView(name: string) {
  navButtons.forEach((b) => b.classList.toggle("active", b.dataset.view === name));
  document.querySelectorAll<HTMLElement>(".view").forEach((v) => {
    v.classList.toggle("active", v.id === `view-${name}`);
  });
  if (name === "history") void refreshHistory();
}

navButtons.forEach((btn) =>
  btn.addEventListener("click", () => showView(btn.dataset.view!))
);

// ============ Recording control ============

let timerId: ReturnType<typeof setInterval> | null = null;
let timerStart = 0;

function startTimer() {
  timerStart = Date.now();
  recTimer.hidden = false;
  recTimer.textContent = "00:00";
  if (timerId) clearInterval(timerId);
  timerId = setInterval(() => {
    const s = Math.floor((Date.now() - timerStart) / 1000);
    const mm = String(Math.floor(s / 60)).padStart(2, "0");
    const ss = String(s % 60).padStart(2, "0");
    recTimer.textContent = `${mm}:${ss}`;
  }, 250);
}

function stopTimer(hide = true) {
  if (timerId) {
    clearInterval(timerId);
    timerId = null;
  }
  if (hide) recTimer.hidden = true;
}

let errorTimer: ReturnType<typeof setTimeout> | null = null;

function setUiStatus(status: string, detail?: string | null) {
  recBtn.className = "rec-btn";
  brandLed.className = "brand-led";
  statusText.parentElement!.classList.remove("error", "attention");
  if (errorTimer) {
    clearTimeout(errorTimer);
    errorTimer = null;
  }

  switch (status) {
    case "Recording":
      isRecording = true;
      recBtn.classList.add("recording");
      brandLed.classList.add("recording");
      statusText.parentElement!.classList.add("attention");
      statusText.textContent = "聆听中";
      startTimer();
      recCancel.hidden = false;
      showLiveCard();
      break;
    case "Recognizing":
      isRecording = false;
      recBtn.classList.add("recognizing");
      brandLed.classList.add("busy");
      statusText.parentElement!.classList.add("attention");
      statusText.textContent = "识别中";
      stopTimer(false);
      recCancel.hidden = true;
      break;
    case "Done":
      isRecording = false;
      recBtn.classList.add("done");
      brandLed.classList.add("done");
      statusText.textContent = "完成";
      stopTimer();
      recCancel.hidden = true;
      errorTimer = setTimeout(() => setUiStatus("Ready"), 2500);
      break;
    case "Error":
      isRecording = false;
      recBtn.classList.add("error");
      statusText.parentElement!.classList.add("error");
      statusText.textContent = detail || "出错了";
      stopTimer();
      recCancel.hidden = true;
      errorTimer = setTimeout(() => setUiStatus("Ready"), 4500);
      break;
    default:
      isRecording = false;
      statusText.textContent = "待命";
      stopTimer();
      recCancel.hidden = true;
      break;
  }
}

function showLiveCard() {
  resultCard.hidden = false;
  resultLabel.textContent = "实时转写";
  resultLabel.classList.add("live");
  resultTime.textContent = "";
  resultText.textContent = "…";
  copyResultBtn.style.visibility = "hidden";
}

function showResultCard(entry: HistoryEntry, label = "听写结果") {
  resultCard.hidden = false;
  resultLabel.textContent = label;
  resultLabel.classList.remove("live");
  resultTime.textContent = formatTime(entry.id);
  resultText.textContent = entry.text;
  copyResultBtn.style.visibility = "";
  resultText.scrollTop = 0;
}

recBtn.addEventListener("click", async () => {
  try {
    await invoke<boolean>("toggle_recording");
    // UI state follows notype://status events.
  } catch (err) {
    setUiStatus("Error", `${err}`);
  }
});

recCancel.addEventListener("click", () => {
  void invoke("cancel_recording");
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && isRecording) {
    void invoke("cancel_recording");
  }
});

copyResultBtn.addEventListener("click", () => {
  const text = resultText.textContent || "";
  if (!text) return;
  void copyText(text, copyResultBtn);
});

async function copyText(text: string, feedbackBtn?: HTMLElement) {
  try {
    await invoke("copy_text_to_clipboard", { text });
    if (feedbackBtn) {
      feedbackBtn.classList.add("flash");
      setTimeout(() => feedbackBtn.classList.remove("flash"), 1200);
    }
  } catch (err) {
    console.error("copy failed:", err);
  }
}

// ============ History ============

function formatTime(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  if (d.toDateString() === now.toDateString()) return `${hh}:${mm}`;
  const mo = String(d.getMonth() + 1).padStart(2, "0");
  const da = String(d.getDate()).padStart(2, "0");
  if (d.getFullYear() === now.getFullYear()) return `${mo}-${da} ${hh}:${mm}`;
  return `${d.getFullYear()}-${mo}-${da} ${hh}:${mm}`;
}

async function refreshHistory() {
  historyCache = await invoke<HistoryEntry[]>("get_history");
  historyLoaded = true;
  renderHistory();
}

function renderHistory() {
  historyList.innerHTML = "";
  historyEmpty.hidden = historyCache.length > 0;
  clearHistoryBtn.style.visibility = historyCache.length > 0 ? "" : "hidden";

  for (const entry of historyCache) {
    const li = document.createElement("li");
    li.className = "history-item";

    const meta = document.createElement("div");
    meta.className = "history-meta";

    const time = document.createElement("span");
    time.className = "history-time mono";
    time.textContent = formatTime(entry.id);
    meta.appendChild(time);

    const tag = document.createElement("span");
    tag.className = "history-tag";
    tag.textContent = entry.provider;
    meta.appendChild(tag);

    if (entry.duration_secs > 0) {
      const dur = document.createElement("span");
      dur.className = "history-tag mono";
      dur.textContent = `${entry.duration_secs.toFixed(1)}s`;
      meta.appendChild(dur);
    }

    const actions = document.createElement("div");
    actions.className = "history-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "icon-btn";
    copyBtn.title = "复制";
    copyBtn.innerHTML =
      '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>';
    copyBtn.addEventListener("click", () => void copyText(entry.text, copyBtn));
    actions.appendChild(copyBtn);

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "icon-btn";
    delBtn.title = "删除";
    delBtn.innerHTML =
      '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>';
    delBtn.addEventListener("click", async () => {
      historyCache = await invoke<HistoryEntry[]>("delete_history_entry", {
        id: entry.id,
      });
      renderHistory();
    });
    actions.appendChild(delBtn);

    meta.appendChild(actions);
    li.appendChild(meta);

    const text = document.createElement("p");
    text.className = "history-text";
    text.textContent = entry.text;
    text.addEventListener("dblclick", () => li.classList.toggle("expanded"));
    li.appendChild(text);

    historyList.appendChild(li);
  }
}

// 两段式清空：先武装，3 秒内再点才执行
let clearArmed = false;
let clearArmTimer: ReturnType<typeof setTimeout> | null = null;

clearHistoryBtn.addEventListener("click", async () => {
  if (!clearArmed) {
    clearArmed = true;
    clearHistoryBtn.textContent = "确认清空？";
    clearHistoryBtn.classList.add("danger-armed");
    clearArmTimer = setTimeout(() => disarmClear(), 3000);
    return;
  }
  disarmClear();
  await invoke("clear_history");
  historyCache = [];
  renderHistory();
});

function disarmClear() {
  clearArmed = false;
  if (clearArmTimer) clearTimeout(clearArmTimer);
  clearHistoryBtn.textContent = "清空";
  clearHistoryBtn.classList.remove("danger-armed");
}

// ============ Prompts ============

function switchTab(tab: keyof Prompts) {
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

$("save-prompt").addEventListener("click", async () => {
  currentPrompts[activeTab] = promptEditor.value;
  promptStatus.textContent = "保存中…";
  promptStatus.style.color = "";
  try {
    await invoke("save_prompts", { dto: currentPrompts });
    promptStatus.textContent = "已保存";
    promptStatus.style.color = "var(--ok)";
    setTimeout(() => {
      promptStatus.textContent = "";
    }, 2000);
  } catch (err) {
    promptStatus.textContent = `${err}`;
    promptStatus.style.color = "var(--signal)";
  }
});

$("reset-prompt").addEventListener("click", async () => {
  const builtins: Prompts = await invoke("get_builtin_prompts");
  currentPrompts[activeTab] = builtins[activeTab];
  promptEditor.value = builtins[activeTab];
  promptStatus.textContent = "已恢复默认";
  promptStatus.style.color = "";
  setTimeout(() => {
    promptStatus.textContent = "";
  }, 2000);
});

// ---- Prompt preview (Pretext canvas) ----

const previewWrap = $("prompt-preview-wrap");
const previewCanvas = $<HTMLCanvasElement>("prompt-preview-canvas");
const togglePreviewBtn = $("toggle-preview");
let previewVisible = false;

const PREVIEW_FONT_SIZE = 11;
const PREVIEW_FONT = `${PREVIEW_FONT_SIZE}px -apple-system, BlinkMacSystemFont, PingFang SC, SF Pro Text, sans-serif`;
const PREVIEW_LINE_HEIGHT = 16;
const PREVIEW_DPR = window.devicePixelRatio || 2;

function cssVar(name: string, fallback: string): string {
  return (
    getComputedStyle(document.documentElement).getPropertyValue(name).trim() ||
    fallback
  );
}

function renderPromptPreview() {
  if (!previewVisible) return;

  currentPrompts[activeTab] = promptEditor.value;
  const parts = [
    currentPrompts.agent,
    currentPrompts.rules,
    currentPrompts.vocabulary,
  ].filter((s) => s.trim().length > 0);
  const composed = parts.join("\n\n");

  if (!composed) {
    previewCanvas.style.display = "none";
    return;
  }
  previewCanvas.style.display = "";

  const padH = 16;
  const padV = 12;
  const cssW = previewCanvas.parentElement!.clientWidth - 36;
  const textW = cssW - padH * 2;

  const prepared = prepareWithSegments(composed, PREVIEW_FONT, {
    whiteSpace: "pre-wrap",
  });
  const result = layoutWithLines(prepared, textW, PREVIEW_LINE_HEIGHT);

  const cssH = result.height + padV * 2;
  previewCanvas.style.width = `${cssW}px`;
  previewCanvas.style.height = `${cssH}px`;
  previewCanvas.width = Math.round(cssW * PREVIEW_DPR);
  previewCanvas.height = Math.round(cssH * PREVIEW_DPR);

  const ctx = previewCanvas.getContext("2d")!;
  ctx.scale(PREVIEW_DPR, PREVIEW_DPR);

  ctx.fillStyle = cssVar("--bg", "#f2eee6");
  ctx.beginPath();
  ctx.roundRect(0, 0, cssW, cssH, 8);
  ctx.fill();

  // 三个模块用三种颜色区分：墨色 / 信号红 / 绿色
  const sectionColors = [
    cssVar("--ink", "#211e19"),
    cssVar("--signal", "#d8442b"),
    cssVar("--ok", "#43824f"),
  ];
  let sectionIdx = 0;
  ctx.font = PREVIEW_FONT;
  ctx.textBaseline = "alphabetic";

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
    const lineStart = charOffset;
    while (
      sectionIdx < sectionStarts.length - 1 &&
      lineStart >= sectionStarts[sectionIdx + 1]
    ) {
      sectionIdx++;
    }
    ctx.fillStyle = sectionColors[sectionIdx % sectionColors.length];

    const y = padV + li * PREVIEW_LINE_HEIGHT + PREVIEW_FONT_SIZE;
    ctx.fillText(result.lines[li].text, padH, y);
    charOffset += result.lines[li].text.length;
    if (li < result.lines.length - 1) charOffset++;
  }
}

togglePreviewBtn.addEventListener("click", () => {
  previewVisible = !previewVisible;
  previewWrap.hidden = !previewVisible;
  togglePreviewBtn.style.color = previewVisible ? "var(--ink)" : "";
  if (previewVisible) renderPromptPreview();
});

let previewTimer: ReturnType<typeof setTimeout> | null = null;
promptEditor.addEventListener("input", () => {
  if (!previewVisible) return;
  if (previewTimer) clearTimeout(previewTimer);
  previewTimer = setTimeout(renderPromptPreview, 300);
});

// ============ Settings ============

function setProvider(value: string) {
  currentProvider = value;
  providerSeg
    .querySelectorAll<HTMLButtonElement>(".seg-btn")
    .forEach((b) => b.classList.toggle("active", b.dataset.value === value));
  syncModels();
}

providerSeg.querySelectorAll<HTMLButtonElement>(".seg-btn").forEach((btn) => {
  btn.addEventListener("click", () => setProvider(btn.dataset.value!));
});

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
  const groups: Record<string, HTMLOptGroupElement> = {
    gemini: $("gemini-models") as unknown as HTMLOptGroupElement,
    qwen: $("qwen-models") as unknown as HTMLOptGroupElement,
    mimo: $("mimo-models") as unknown as HTMLOptGroupElement,
    doubao: $("doubao-models") as unknown as HTMLOptGroupElement,
  };
  for (const [key, group] of Object.entries(groups)) {
    group.style.display = key === currentProvider ? "" : "none";
  }

  const prefixes: Record<string, string> = {
    gemini: "gemini",
    qwen: "qwen",
    mimo: "mimo",
    doubao: "doubao-asr",
  };
  const defaults: Record<string, string> = {
    gemini: "gemini-3-flash-preview",
    qwen: "qwen3.5-omni-flash",
    mimo: "mimo-v2.5-asr",
    doubao: "doubao-asr",
  };
  if (!modelEl.value.startsWith(prefixes[currentProvider])) {
    modelEl.value = defaults[currentProvider];
  }

  $("gemini-key-field").style.display = currentProvider === "gemini" ? "" : "none";
  $("qwen-key-field").style.display = currentProvider === "qwen" ? "" : "none";
  $("mimo-fields").hidden = currentProvider !== "mimo";
  $("doubao-fields").hidden = currentProvider !== "doubao";
  $("doubao-official-key-fields").hidden = !(
    currentProvider === "doubao" && isDoubaoOfficialModel(modelEl.value)
  );
}

function updateDoubaoPostprocessHint() {
  const enabled = doubaoPostprocessEnabledEl.checked;
  const preferred = doubaoPostprocessProviderEl.value;
  const realtimeWs = doubaoRealtimeWsEnabledEl.checked;
  const preferredLabel =
    preferred === "qwen" ? "Qwen" : preferred === "gemini" ? "Gemini" : "自动（Qwen → Gemini）";
  const wsLabel = realtimeWs ? "WS 实时预览已开启" : "WS 实时预览已关闭";

  doubaoPostprocessHint.className = "s-hint";
  if (!enabled) {
    doubaoPostprocessHint.textContent = `${wsLabel}；已关闭 LLM 后处理，仅输出 ASR 粗转写`;
    return;
  }
  if (hasQwenKey || hasGeminiKey) {
    doubaoPostprocessHint.textContent = `${wsLabel}；实时后处理已启用（偏好 ${preferredLabel}）`;
    doubaoPostprocessHint.classList.add("ok");
  } else {
    doubaoPostprocessHint.textContent = `${wsLabel}；已启用但未配置 Qwen / Gemini Key，将退化为纯 ASR`;
  }
}

function setKeyHint(
  input: HTMLInputElement,
  hint: HTMLElement,
  configured: boolean,
  optional = false
) {
  hint.className = "s-hint";
  if (configured) {
    input.placeholder = "••••••••";
    hint.textContent = "已配置";
    hint.classList.add("ok");
  } else {
    hint.textContent = optional ? "可选" : "未配置";
  }
}

modelEl.addEventListener("change", syncModels);
doubaoPostprocessEnabledEl.addEventListener("change", updateDoubaoPostprocessHint);
doubaoPostprocessProviderEl.addEventListener("change", updateDoubaoPostprocessHint);
doubaoRealtimeWsEnabledEl.addEventListener("change", updateDoubaoPostprocessHint);

document.querySelectorAll(".toggle-key-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    const input = $<HTMLInputElement>((btn as HTMLElement).dataset.target!);
    input.type = input.type === "password" ? "text" : "password";
  });
});

// ---- Hotkey capture ----

hotkeyEl.addEventListener("beforeinput", (e) => e.preventDefault());
hotkeyEl.addEventListener("paste", (e) => e.preventDefault());
hotkeyEl.addEventListener("focus", () => {
  hotkeyEl.value = "按下组合键…";
});

hotkeyEl.addEventListener("keydown", (e) => {
  e.preventDefault();
  e.stopPropagation();

  if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
  if (!e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey) return;

  const parts: string[] = [];
  if (e.metaKey) parts.push("Cmd");
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");

  const key = codeToKeyName(e.code);
  if (!key) return;
  parts.push(key);

  hotkeyEl.value = parts.join("+");
  hotkeyEl.blur();
});

hotkeyEl.addEventListener("blur", () => {
  if (hotkeyEl.value === "按下组合键…") {
    hotkeyEl.value = hotkeyEl.dataset.prev || "Ctrl+.";
  } else {
    hotkeyEl.dataset.prev = hotkeyEl.value;
  }
});

/** KeyboardEvent.code → 稳定的物理键名（e.key 在带修饰键时可能乱码） */
function codeToKeyName(code: string): string | null {
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
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

// ---- Autostart（即时生效，不走保存） ----

autostartEl.addEventListener("change", async () => {
  try {
    await invoke("set_autostart", { enabled: autostartEl.checked });
  } catch (err) {
    autostartEl.checked = !autostartEl.checked;
    saveStatus.textContent = `开机自启设置失败：${err}`;
    saveStatus.className = "s-msg bad";
  }
});

$("open-config-dir").addEventListener("click", () => {
  void invoke("open_config_dir");
});

// ---- Save settings ----

settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  saveStatus.textContent = "保存中…";
  saveStatus.className = "s-msg";

  try {
    const result = await invoke<SaveResult>("save_config", {
      dto: {
        provider: currentProvider,
        gemini_api_key: geminiKeyEl.value,
        qwen_api_key: qwenKeyEl.value,
        mimo_api_key: mimoKeyEl.value,
        mimo_base_url: mimoBaseUrlEl.value,
        doubao_api_key: doubaoKeyEl.value,
        doubao_base_url: doubaoBaseUrlEl.value,
        doubao_official_app_key: doubaoOfficialAppKeyEl.value,
        doubao_official_access_key: doubaoOfficialAccessKeyEl.value,
        enable_doubao_postprocess: doubaoPostprocessEnabledEl.checked,
        doubao_postprocess_provider: doubaoPostprocessProviderEl.value,
        enable_doubao_realtime_ws: doubaoRealtimeWsEnabledEl.checked,
        doubao_ime_credential_path: doubaoImeCredentialPathEl.value,
        model_name: modelEl.value,
        hotkey: hotkeyEl.value,
        audio_device: audioDeviceEl.value,
        input_mode: inputModeEl.value,
        auto_copy: autoCopyEl.checked,
        has_gemini_key: true,
        has_qwen_key: true,
        has_mimo_key: true,
        has_doubao_key: true,
        has_doubao_official_app_key: true,
        has_doubao_official_access_key: true,
      },
    });

    if (result.restart_needed) {
      saveStatus.textContent = "已保存，快捷键需重启应用后生效";
      saveStatus.className = "s-msg bad";
    } else {
      saveStatus.textContent = "已保存";
      saveStatus.className = "s-msg ok";
      setTimeout(() => {
        saveStatus.textContent = "";
      }, 2000);
    }

    const hintEl = $("hotkey-hint");
    hintEl.textContent = hotkeyEl.value.replace(/\+/g, " + ");

    if (geminiKeyEl.value) {
      geminiKeyEl.value = "";
      hasGeminiKey = true;
      setKeyHint(geminiKeyEl, geminiKeyHint, true);
    }
    if (qwenKeyEl.value) {
      qwenKeyEl.value = "";
      hasQwenKey = true;
      setKeyHint(qwenKeyEl, qwenKeyHint, true);
    }
    if (mimoKeyEl.value) {
      mimoKeyEl.value = "";
      setKeyHint(mimoKeyEl, mimoKeyHint, true);
    }
    if (doubaoKeyEl.value) {
      doubaoKeyEl.value = "";
      setKeyHint(doubaoKeyEl, doubaoKeyHint, true, true);
    }
    if (doubaoOfficialAppKeyEl.value) {
      doubaoOfficialAppKeyEl.value = "";
      setKeyHint(doubaoOfficialAppKeyEl, doubaoOfficialAppKeyHint, true);
    }
    if (doubaoOfficialAccessKeyEl.value) {
      doubaoOfficialAccessKeyEl.value = "";
      setKeyHint(doubaoOfficialAccessKeyEl, doubaoOfficialAccessKeyHint, true);
    }

    updateDoubaoPostprocessHint();
  } catch (err) {
    saveStatus.textContent = `${err}`;
    saveStatus.className = "s-msg bad";
  }
});

// ---- Doubao one-click setup ----

doubaoSetupBtn.addEventListener("click", async () => {
  doubaoSetupBtn.disabled = true;
  doubaoSetupStatus.textContent = "正在安装依赖、初始化凭证并启动本地网关…";
  doubaoSetupStatus.className = "s-hint";

  try {
    const desiredPath = doubaoImeCredentialPathEl.value.trim();
    const desiredBaseUrl = doubaoBaseUrlEl.value.trim();
    const desiredGatewayApiKey = doubaoKeyEl.value.trim();
    const result = await invoke<DoubaoRealtimeSetupResult>(
      "setup_doubao_realtime_runtime",
      {
        credentialPath: desiredPath || null,
        baseUrl: desiredBaseUrl || null,
        gatewayApiKey: desiredGatewayApiKey || null,
      }
    );
    doubaoImeCredentialPathEl.value = result.credential_path;
    doubaoBaseUrlEl.value = result.base_url;
    setProvider("doubao");
    modelEl.value = result.model_name || "doubao-asr";
    syncModels();
    if (desiredGatewayApiKey) {
      doubaoKeyEl.value = "";
      setKeyHint(doubaoKeyEl, doubaoKeyHint, true, true);
    }
    doubaoSetupStatus.textContent = `就绪（${result.python}）：${result.message}`;
    doubaoSetupStatus.className = "s-hint ok";
  } catch (err) {
    doubaoSetupStatus.textContent = `${err}`;
    doubaoSetupStatus.className = "s-hint bad";
  } finally {
    doubaoSetupBtn.disabled = false;
  }
});

// ============ Init ============

async function init() {
  const version: string = await invoke("get_version");
  versionEl.textContent = `v${version}`;

  const config: Config = await invoke("get_config");
  setProvider(config.provider);
  modelEl.value = config.model_name;
  mimoBaseUrlEl.value = config.mimo_base_url || "https://api.xiaomimimo.com/v1";
  doubaoBaseUrlEl.value = config.doubao_base_url || "http://127.0.0.1:8000";
  doubaoPostprocessEnabledEl.checked = config.enable_doubao_postprocess;
  doubaoPostprocessProviderEl.value = config.doubao_postprocess_provider || "auto";
  doubaoRealtimeWsEnabledEl.checked = config.enable_doubao_realtime_ws;
  doubaoImeCredentialPathEl.value =
    config.doubao_ime_credential_path || "~/.config/doubaoime-asr/credentials.json";
  hotkeyEl.value = config.hotkey;
  hotkeyEl.dataset.prev = config.hotkey;
  inputModeEl.value = config.input_mode === "clipboard" ? "clipboard" : "keyboard";
  autoCopyEl.checked = config.auto_copy;

  $("hotkey-hint").textContent = config.hotkey.replace(/\+/g, " + ");

  hasGeminiKey = config.has_gemini_key;
  hasQwenKey = config.has_qwen_key;

  setKeyHint(geminiKeyEl, geminiKeyHint, config.has_gemini_key);
  setKeyHint(qwenKeyEl, qwenKeyHint, config.has_qwen_key);
  setKeyHint(mimoKeyEl, mimoKeyHint, config.has_mimo_key);
  setKeyHint(doubaoKeyEl, doubaoKeyHint, config.has_doubao_key, true);
  setKeyHint(
    doubaoOfficialAppKeyEl,
    doubaoOfficialAppKeyHint,
    config.has_doubao_official_app_key
  );
  setKeyHint(
    doubaoOfficialAccessKeyEl,
    doubaoOfficialAccessKeyHint,
    config.has_doubao_official_access_key
  );

  updateDoubaoPostprocessHint();
  syncModels();

  // Prompts
  currentPrompts = await invoke("get_prompts");
  promptEditor.value = currentPrompts[activeTab];

  // 当前服务商缺少凭证时直接进设置页
  const needsKey =
    (config.provider === "gemini" && !config.has_gemini_key) ||
    (config.provider === "qwen" && !config.has_qwen_key) ||
    (config.provider === "mimo" && !config.has_mimo_key) ||
    (config.provider === "doubao" &&
      isDoubaoOfficialModel(config.model_name) &&
      (!config.has_doubao_official_app_key || !config.has_doubao_official_access_key));
  if (needsKey) showView("settings");

  // 麦克风设备
  const devices: AudioDevice[] = await invoke("list_audio_devices");
  audioDeviceEl.innerHTML = "";
  const defaultOpt = document.createElement("option");
  defaultOpt.value = "";
  defaultOpt.textContent = "系统默认";
  audioDeviceEl.appendChild(defaultOpt);
  for (const d of devices) {
    const opt = document.createElement("option");
    opt.value = d.name;
    opt.textContent = d.is_default ? `${d.name}（默认）` : d.name;
    audioDeviceEl.appendChild(opt);
  }
  audioDeviceEl.value = config.audio_device || "";
  if (audioDeviceEl.value !== (config.audio_device || "")) {
    // 已保存的设备当前不在线 → 回退显示系统默认
    audioDeviceEl.value = "";
  }

  // 开机自启当前状态
  try {
    autostartEl.checked = await invoke<boolean>("get_autostart");
  } catch {
    autostartEl.checked = false;
  }

  // 历史 + 主页最近一条
  await refreshHistory();
  if (historyCache.length > 0) {
    showResultCard(historyCache[0], "上次听写");
  }

  // 事件订阅
  await listen<StatusEvent>("notype://status", (e) => {
    setUiStatus(e.payload.status, e.payload.detail);
  });

  await listen<string>("notype://interim", (e) => {
    if (!resultCard.hidden && resultLabel.classList.contains("live")) {
      resultText.textContent = e.payload || "…";
      resultText.scrollTop = resultText.scrollHeight;
    }
  });

  await listen<HistoryEntry>("notype://result", (e) => {
    const entry = e.payload;
    showResultCard(entry, "听写结果");
    if (historyLoaded) {
      historyCache.unshift(entry);
      renderHistory();
    }
  });
}

window.addEventListener("DOMContentLoaded", () => {
  void init();
});
