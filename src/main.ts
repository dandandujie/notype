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
  qwen_base_url: string;
  volc_app_key: string;
  volc_access_key: string;
  volc_resource_id: string;
  whisper_base_url: string;
  whisper_api_key: string;
  whisper_model: string;
  apple_locale: string;
  enable_postprocess: boolean;
  postprocess_provider: string;
  custom_llm_base_url: string;
  custom_llm_api_key: string;
  custom_llm_model: string;
  structured_output: boolean;
  stream_typing: boolean;
  sound_feedback: boolean;
  auto_enter: boolean;
  app_rules: string;
  onboarded: boolean;
  model_name: string;
  hotkey: string;
  edit_hotkey: string;
  audio_device: string;
  input_mode: string;
  auto_copy: boolean;
  output_style: string;
  enable_app_context: boolean;
  has_gemini_key: boolean;
  has_qwen_key: boolean;
  has_mimo_key: boolean;
  has_volc_keys: boolean;
  has_whisper_key: boolean;
  has_custom_llm_key: boolean;
}

interface Prompts {
  agent: string;
  rules: string;
  vocabulary: string;
  replace_rules: string;
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

interface Stats {
  total_chars: number;
  total_duration_secs: number;
  total_sessions: number;
  streak_days: number;
  learned_pairs: number;
}

interface VocabPair {
  wrong: string;
  right: string;
}

interface Permissions {
  accessibility: boolean;
}

interface SaveResult {
  restart_needed: boolean;
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
const editResultBtn = $("edit-result");

const styleSeg = $("style-seg");
const persoBar = $("perso-bar");
const persoFill = $("perso-fill");
const persoPct = $("perso-pct");
const statsStrip = $("stats-strip");
const statChars = $("stat-chars");
const statSpeed = $("stat-speed");
const statSaved = $("stat-saved");
const statStreak = $("stat-streak");

const historyList = $("history-list");
const historyEmpty = $("history-empty");
const clearHistoryBtn = $("clear-history");
const exportHistoryBtn = $("export-history");
const historySearchEl = $<HTMLInputElement>("history-search");
const setupNudge = $("setup-nudge");

const vocabQuick = $("vocab-quick");
const vocabWrong = $<HTMLInputElement>("vocab-wrong");
const vocabRight = $<HTMLInputElement>("vocab-right");

const appContextEl = $<HTMLInputElement>("app-context");
const permRow = $("perm-row");

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
const qwenBaseUrlEl = $<HTMLInputElement>("qwen-base-url");
const volcAppKeyEl = $<HTMLInputElement>("volc-app-key");
const volcAppKeyHint = $("volc-app-key-hint");
const volcAccessKeyEl = $<HTMLInputElement>("volc-access-key");
const volcAccessKeyHint = $("volc-access-key-hint");
const volcResourceIdEl = $<HTMLInputElement>("volc-resource-id");
const whisperBaseUrlEl = $<HTMLInputElement>("whisper-base-url");
const whisperApiKeyEl = $<HTMLInputElement>("whisper-api-key");
const whisperKeyHint = $("whisper-key-hint");
const whisperModelEl = $<HTMLInputElement>("whisper-model");
const appleLocaleEl = $<HTMLInputElement>("apple-locale");
const postprocessEnabledEl = $<HTMLInputElement>("postprocess-enabled");
const structuredOutputEl = $<HTMLInputElement>("structured-output");
const postprocessProviderEl = $<HTMLSelectElement>("postprocess-provider");
const customLlmFields = $("custom-llm-fields");
const customLlmBaseUrlEl = $<HTMLInputElement>("custom-llm-base-url");
const customLlmApiKeyEl = $<HTMLInputElement>("custom-llm-api-key");
const customLlmKeyHint = $("custom-llm-key-hint");
const customLlmModelEl = $<HTMLInputElement>("custom-llm-model");
const streamTypingEl = $<HTMLInputElement>("stream-typing");
const soundFeedbackEl = $<HTMLInputElement>("sound-feedback");
const autoEnterEl = $<HTMLInputElement>("auto-enter");
const appRulesEl = $<HTMLTextAreaElement>("app-rules");

const hotkeyEl = $<HTMLInputElement>("hotkey");
const editHotkeyEl = $<HTMLInputElement>("edit-hotkey");
const audioDeviceEl = $<HTMLSelectElement>("audio-device");
const inputModeEl = $<HTMLSelectElement>("input-mode");
const autoCopyEl = $<HTMLInputElement>("auto-copy");
const autostartEl = $<HTMLInputElement>("autostart");
const settingsForm = $<HTMLFormElement>("settings-form");
const saveStatus = $("save-status");
const versionEl = $("version");

// ============ State ============

let currentProvider = "gemini";
let currentStyle = "polish";
let currentPrompts: Prompts = { agent: "", rules: "", vocabulary: "", replace_rules: "" };
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

// ============ 声音反馈（合成音，无资源文件） ============

let soundEnabled = true;
let audioCtx: AudioContext | null = null;

function playTone(freq: number, dur = 0.09, gain = 0.045, type: OscillatorType = "sine", delay = 0) {
  if (!soundEnabled) return;
  try {
    audioCtx ??= new AudioContext();
    if (audioCtx.state === "suspended") {
      void audioCtx.resume();
      if (audioCtx.state === "suspended") return; // 无法解锁则静默
    }
    const t = audioCtx.currentTime + delay;
    const osc = audioCtx.createOscillator();
    const g = audioCtx.createGain();
    osc.type = type;
    osc.frequency.value = freq;
    g.gain.setValueAtTime(gain, t);
    g.gain.exponentialRampToValueAtTime(0.0001, t + dur);
    osc.connect(g).connect(audioCtx.destination);
    osc.start(t);
    osc.stop(t + dur);
  } catch {
    /* best effort */
  }
}

const sounds = {
  start: () => playTone(659, 0.07),
  done: () => {
    playTone(784, 0.07);
    playTone(1175, 0.11, 0.04, "sine", 0.07);
  },
  error: () => playTone(196, 0.16, 0.04, "square"),
};

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
      sounds.start();
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
      resetWaveform();
      recCancel.hidden = true;
      break;
    case "Done":
      isRecording = false;
      sounds.done();
      recBtn.classList.add("done");
      brandLed.classList.add("done");
      statusText.textContent = "完成";
      stopTimer();
      resetWaveform();
      recCancel.hidden = true;
      errorTimer = setTimeout(() => setUiStatus("Ready"), 2500);
      break;
    case "Error":
      isRecording = false;
      sounds.error();
      recBtn.classList.add("error");
      statusText.parentElement!.classList.add("error");
      statusText.textContent = detail || "出错了";
      stopTimer();
      resetWaveform();
      recCancel.hidden = true;
      errorTimer = setTimeout(() => setUiStatus("Ready"), 4500);
      break;
    default:
      isRecording = false;
      statusText.textContent = "待命";
      stopTimer();
      resetWaveform();
      recCancel.hidden = true;
      break;
  }

  // Onboarding 试听写按钮镜像主录音键状态
  const obRec = document.getElementById("ob-rec");
  if (obRec && !$("onboard").hidden) {
    obRec.className = `rec-btn ob-rec ${recBtn.classList.contains("recording") ? "recording" : ""} ${recBtn.classList.contains("recognizing") ? "recognizing" : ""} ${recBtn.classList.contains("done") ? "done" : ""}`.trim();
  }
}

let currentResultId: number | null = null;

function showLiveCard() {
  cancelResultEdit();
  currentResultId = null;
  resultCard.hidden = false;
  resultLabel.textContent = "实时转写";
  resultLabel.classList.add("live");
  resultTime.textContent = "";
  resultText.textContent = "…";
  copyResultBtn.style.visibility = "hidden";
  editResultBtn.style.visibility = "hidden";
}

function showResultCard(entry: HistoryEntry, label = "听写结果") {
  cancelResultEdit();
  currentResultId = entry.id;
  resultCard.hidden = false;
  resultLabel.textContent = label;
  resultLabel.classList.remove("live");
  resultTime.textContent = formatTime(entry.id);
  resultText.textContent = entry.text;
  copyResultBtn.style.visibility = "";
  editResultBtn.style.visibility = "";
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

// ---- 输出风格快速切换 ----

function setStyleSeg(value: string) {
  currentStyle = value;
  styleSeg
    .querySelectorAll<HTMLButtonElement>(".seg-btn")
    .forEach((b) => b.classList.toggle("active", b.dataset.style === value));
}

styleSeg.querySelectorAll<HTMLButtonElement>(".seg-btn").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const style = btn.dataset.style!;
    setStyleSeg(style);
    try {
      await invoke("set_output_style", { style });
    } catch (err) {
      console.error("set_output_style failed:", err);
    }
  });
});

// ---- 真实音量驱动波形 ----

const waveformEl = recBtn.querySelector<HTMLElement>(".waveform")!;
const waveBars = Array.from(waveformEl.querySelectorAll<HTMLElement>("i"));
// 中间高、两侧低的权重，模拟频谱形状
const WAVE_WEIGHTS = [0.45, 0.62, 0.85, 1, 0.9, 1, 0.85, 0.62, 0.45];
let smoothedLevel = 0;

function driveWaveform(level: number) {
  if (!recBtn.classList.contains("recording")) return;
  // RMS 一般 0.02~0.25，放大到 0..1 并平滑
  const amp = Math.min(1, level * 7);
  smoothedLevel = smoothedLevel * 0.55 + amp * 0.45;
  waveformEl.classList.add("live");
  waveBars.forEach((bar, i) => {
    const jitter = 0.82 + Math.random() * 0.36;
    const h = 4 + WAVE_WEIGHTS[i] * smoothedLevel * 19 * jitter;
    bar.style.height = `${Math.max(3, Math.min(23, h))}px`;
  });
}

function resetWaveform() {
  smoothedLevel = 0;
  waveformEl.classList.remove("live");
  waveBars.forEach((bar) => {
    bar.style.height = "";
  });
}

// ---- 统计仪表 ----

function fmtChars(n: number): string {
  if (n >= 100_000_000) return `${(n / 100_000_000).toFixed(1)} 亿`;
  if (n >= 10_000) return `${(n / 10_000).toFixed(1)} 万`;
  return `${n}`;
}

function renderStats(stats: Stats) {
  lastStats = stats;
  renderPersonalization();
  if (stats.total_sessions <= 0) {
    statsStrip.hidden = true;
    return;
  }
  statsStrip.hidden = false;
  setStat(statChars, fmtChars(stats.total_chars));

  const minutes = stats.total_duration_secs / 60;
  setStat(statSpeed, minutes >= 0.5 ? `${Math.round(stats.total_chars / minutes)}` : "–");

  // 省时估算：手打 40 字/分 vs 实际说话用时
  const savedMin = Math.max(0, stats.total_chars / 40 - minutes);
  setStat(
    statSaved,
    savedMin >= 90 ? `${(savedMin / 60).toFixed(1)} 小时` : `${Math.round(savedMin)} 分钟`
  );

  setStat(statStreak, `${stats.streak_days}`);
}

/** 数值变化时触发一次轻弹动画 */
function setStat(el: HTMLElement, value: string) {
  if (el.textContent === value) return;
  el.textContent = value;
  el.classList.remove("tick");
  void el.offsetWidth; // 重启动画
  el.classList.add("tick");
}

recCancel.addEventListener("click", () => {
  void invoke("cancel_recording");
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && isRecording) {
    void invoke("cancel_recording");
  }
});

setupNudge.addEventListener("click", () => showView("settings"));

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

// ============ 纠错学习（词典自动学习） ============

/** 按句切分（保留边界符），用于句级对齐 diff */
function splitSentences(text: string): string[] {
  const out: string[] = [];
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    if ("。！？；.!?;\n".includes(text[i])) {
      out.push(text.slice(start, i + 1));
      start = i + 1;
    }
  }
  if (start < text.length) out.push(text.slice(start));
  return out;
}

/**
 * 从「原文 → 用户修改后」提取纠错词对。
 * 策略：句数一致时逐句对齐，各句做前后缀裁剪取中间替换段；
 * 否则整体做一次裁剪。词对两侧 ≤12 字、非纯标点才算有效。
 */
function extractCorrections(original: string, edited: string): VocabPair[] {
  if (original === edited) return [];
  const pairs: VocabPair[] = [];
  const seen = new Set<string>();

  const pushDiff = (a: string, b: string) => {
    if (a === b) return;
    let i = 0;
    while (i < a.length && i < b.length && a[i] === b[i]) i++;
    let j = 0;
    while (
      j < a.length - i &&
      j < b.length - i &&
      a[a.length - 1 - j] === b[b.length - 1 - j]
    ) {
      j++;
    }
    const rawWrong = a.slice(i, a.length - j).trim();
    const rawRight = b.slice(i, b.length - j).trim();
    // 纯插入/删除不是「听错」；纯标点/空白修改也不值得学
    if (!rawWrong || !rawRight) return;
    if (/^[\p{P}\p{S}\s]+$/u.test(rawWrong) || /^[\p{P}\p{S}\s]+$/u.test(rawRight)) return;

    // 单字差异（如「含→函」）缺少上下文，向相邻共同字符扩一位
    // 使词对成为「含数→函数」这样可用的纠错单元
    const span = (s: string) => [...s.slice(i, s.length - j).trim()].length;
    while ((span(a) < 2 || span(b) < 2) && (j > 0 || i > 0)) {
      if (j > 0) j--;
      else i--;
    }
    const wrong = a.slice(i, a.length - j).trim();
    const right = b.slice(i, b.length - j).trim();
    if (!wrong || !right || wrong === right) return;
    if ([...wrong].length > 12 || [...right].length > 12) return;
    // 大跨度且零共同字符 → 更像整句重写而非听错，跳过
    const wrongChars = new Set([...wrong]);
    const shared = [...right].some((ch) => wrongChars.has(ch));
    if (!shared && Math.max([...wrong].length, [...right].length) > 8) return;
    const key = `${wrong}→${right}`;
    if (seen.has(key)) return;
    seen.add(key);
    pairs.push({ wrong, right });
  };

  const segsA = splitSentences(original);
  const segsB = splitSentences(edited);
  if (segsA.length === segsB.length && segsA.length > 1) {
    for (let k = 0; k < segsA.length; k++) pushDiff(segsA[k], segsB[k]);
  } else {
    pushDiff(original, edited);
  }
  return pairs.slice(0, 8);
}

/** 提交一次纠错：更新历史 + 自动学习词对，返回学到的数量 */
async function commitCorrection(
  id: number | null,
  original: string,
  edited: string
): Promise<number> {
  if (id != null && edited !== original) {
    try {
      historyCache = await invoke<HistoryEntry[]>("update_history_entry", {
        id,
        text: edited,
      });
      if (historyLoaded) renderHistory();
    } catch (err) {
      console.error("update_history_entry failed:", err);
    }
  }

  const pairs = extractCorrections(original, edited);
  if (pairs.length === 0) return 0;
  try {
    const added = await invoke<number>("learn_vocabulary", { pairs });
    if (added > 0) {
      // 词典变了 → 刷新提示词缓存和个性化进度
      currentPrompts = await invoke("get_prompts");
      if (activeTab === "vocabulary") promptEditor.value = currentPrompts.vocabulary;
      renderPersonalization();
    }
    return added;
  } catch (err) {
    console.error("learn_vocabulary failed:", err);
    return 0;
  }
}

/** 通用的就地编辑：contentEditable + 保存/取消操作条 */
let activeEditCleanup: (() => void) | null = null;

function cancelResultEdit() {
  if (activeEditCleanup) {
    activeEditCleanup();
    activeEditCleanup = null;
  }
}

function makeEditable(
  textEl: HTMLElement,
  original: string,
  onCommit: (edited: string) => void
) {
  cancelResultEdit();

  textEl.contentEditable = "plaintext-only";
  textEl.classList.add("editing");
  textEl.focus();

  const actions = document.createElement("div");
  actions.className = "edit-actions";

  const save = document.createElement("button");
  save.type = "button";
  save.className = "btn-primary btn-sm";
  save.style.marginLeft = "0";
  save.textContent = "保存并学习";

  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn-text";
  cancel.textContent = "取消";

  const hint = document.createElement("span");
  hint.className = "hint";
  hint.textContent = "修改处会自动学进词典";

  actions.append(save, cancel, hint);
  textEl.insertAdjacentElement("afterend", actions);

  const cleanup = (restore: boolean) => {
    textEl.contentEditable = "false";
    textEl.classList.remove("editing");
    if (restore) textEl.textContent = original;
    actions.remove();
    textEl.removeEventListener("keydown", onKey);
    activeEditCleanup = null;
  };

  const commit = () => {
    const edited = (textEl.textContent || "").trim();
    cleanup(false);
    if (edited && edited !== original) {
      textEl.textContent = edited;
      onCommit(edited);
    } else {
      textEl.textContent = original;
    }
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      cleanup(true);
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      commit();
    }
  };

  textEl.addEventListener("keydown", onKey);
  save.addEventListener("click", commit);
  cancel.addEventListener("click", () => cleanup(true));

  activeEditCleanup = () => cleanup(true);
}

function flashResultLabel(msg: string) {
  const prev = resultLabel.textContent;
  resultLabel.textContent = msg;
  setTimeout(() => {
    if (resultLabel.textContent === msg) resultLabel.textContent = prev;
  }, 2600);
}

editResultBtn.addEventListener("click", () => {
  if (resultText.classList.contains("editing")) return;
  const original = resultText.textContent || "";
  if (!original.trim()) return;
  makeEditable(resultText, original, (edited) => {
    void commitCorrection(currentResultId, original, edited).then((added) => {
      flashResultLabel(added > 0 ? `已学习 ${added} 处纠错 → 词典` : "已保存修改");
    });
  });
});

// ============ 个性化进度（越用越懂你） ============

let builtinPromptsCache: Prompts | null = null;
let lastStats: Stats | null = null;

function computePersonalization(): { pct: number; detail: string } {
  const vocabCount = (currentPrompts.vocabulary.match(/→/g) || []).length;
  const learned = lastStats ? Number(lastStats.learned_pairs) : 0;
  const sessions = lastStats ? lastStats.total_sessions : 0;

  let score = Math.min(45, vocabCount * 3);
  score += Math.min(20, learned * 2);
  score += Math.min(20, sessions / 5);
  if (builtinPromptsCache) {
    const agent = currentPrompts.agent.trim();
    const rules = currentPrompts.rules.trim();
    if (agent && agent !== builtinPromptsCache.agent.trim()) score += 8;
    if (rules && rules !== builtinPromptsCache.rules.trim()) score += 7;
  }

  return {
    pct: Math.min(100, Math.round(score)),
    detail: `词典 ${vocabCount} 条 · 自动学习 ${learned} 处 · 听写 ${sessions} 次`,
  };
}

function renderPersonalization() {
  if (!lastStats && !currentPrompts.vocabulary) {
    persoBar.hidden = true;
    return;
  }
  const { pct, detail } = computePersonalization();
  persoBar.hidden = false;
  persoFill.style.width = `${pct}%`;
  persoPct.textContent = `${pct}%`;
  persoBar.title = `${detail} — 点击管理词典`;
}

persoBar.addEventListener("click", () => {
  showView("prompts");
  switchTab("vocabulary");
});

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

let historyQuery = "";

/** 分组标题：今天 / 昨天 / MM-DD / YYYY-MM-DD */
function dayLabel(ms: number): string {
  const d = new Date(ms);
  const now = new Date();
  const startOfDay = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000);
  if (diffDays === 0) return "今天";
  if (diffDays === 1) return "昨天";
  const mo = String(d.getMonth() + 1).padStart(2, "0");
  const da = String(d.getDate()).padStart(2, "0");
  if (d.getFullYear() === now.getFullYear()) return `${mo}-${da}`;
  return `${d.getFullYear()}-${mo}-${da}`;
}

function renderHistory() {
  historyList.innerHTML = "";
  const query = historyQuery.trim().toLowerCase();
  const filtered = query
    ? historyCache.filter((e) => e.text.toLowerCase().includes(query))
    : historyCache;

  historyEmpty.hidden = filtered.length > 0;
  const emptyText = historyEmpty.querySelector("p");
  if (emptyText) {
    emptyText.textContent = query && historyCache.length > 0 ? "没有匹配的记录" : "还没有转写记录";
  }
  clearHistoryBtn.style.visibility = historyCache.length > 0 ? "" : "hidden";
  exportHistoryBtn.style.visibility = historyCache.length > 0 ? "" : "hidden";

  let lastDay = "";
  for (const entry of filtered) {
    const day = dayLabel(entry.id);
    if (day !== lastDay) {
      lastDay = day;
      const header = document.createElement("li");
      header.className = "history-day mono";
      header.textContent = day;
      historyList.appendChild(header);
    }
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

    const editBtn = document.createElement("button");
    editBtn.type = "button";
    editBtn.className = "icon-btn";
    editBtn.title = "纠错（修改会自动学进词典）";
    editBtn.innerHTML =
      '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/></svg>';
    actions.appendChild(editBtn);

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

    editBtn.addEventListener("click", () => {
      if (text.classList.contains("editing")) return;
      li.classList.add("expanded");
      const original = entry.text;
      makeEditable(text, original, (edited) => {
        void commitCorrection(entry.id, original, edited);
        // 若正好是主页展示的那条，同步主页卡片
        if (currentResultId === entry.id) {
          resultText.textContent = edited;
        }
      });
    });

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

historySearchEl.addEventListener("input", () => {
  historyQuery = historySearchEl.value;
  renderHistory();
});

exportHistoryBtn.addEventListener("click", async () => {
  try {
    const path = await invoke<string>("export_history");
    exportHistoryBtn.textContent = "已导出到下载 ✓";
    exportHistoryBtn.title = path;
  } catch (err) {
    exportHistoryBtn.textContent = `${err}`;
  }
  setTimeout(() => {
    exportHistoryBtn.textContent = "导出";
  }, 3000);
});

function disarmClear() {
  clearArmed = false;
  if (clearArmTimer) clearTimeout(clearArmTimer);
  clearHistoryBtn.textContent = "清空";
  clearHistoryBtn.classList.remove("danger-armed");
}

// ============ Prompts ============

const TAB_PLACEHOLDERS: Record<keyof Prompts, string> = {
  agent: "在此编辑提示词…",
  rules: "在此编辑转录规则…",
  vocabulary: "在此编辑专有词汇…",
  replace_rules:
    "确定性替换规则，识别完成后强制应用（LLM 之外）。每行一条：\n含数 = 函数\n/(\\d+)块(\\d+)/ = $1.$2元\n# 井号开头是注释",
};

function switchTab(tab: keyof Prompts) {
  currentPrompts[activeTab] = promptEditor.value;
  activeTab = tab;
  promptEditor.value = currentPrompts[tab];
  promptEditor.placeholder = TAB_PLACEHOLDERS[tab];
  vocabQuick.hidden = tab !== "vocabulary";
  document.querySelectorAll(".tab").forEach((t) => {
    t.classList.toggle("active", (t as HTMLElement).dataset.tab === tab);
  });
}

// 词典快速添加：追加一行「错 → 对」并立即保存
$("vocab-add").addEventListener("click", async () => {
  const wrong = vocabWrong.value.trim();
  const right = vocabRight.value.trim();
  if (!wrong || !right) return;

  currentPrompts[activeTab] = promptEditor.value;
  const line = `- ${wrong} → ${right}`;
  currentPrompts.vocabulary = currentPrompts.vocabulary.trimEnd() + "\n" + line;
  if (activeTab === "vocabulary") {
    promptEditor.value = currentPrompts.vocabulary;
    promptEditor.scrollTop = promptEditor.scrollHeight;
  }

  try {
    await invoke("save_prompts", { dto: currentPrompts });
    promptStatus.textContent = "已添加并保存";
    promptStatus.style.color = "var(--ok)";
    setTimeout(() => {
      promptStatus.textContent = "";
    }, 2000);
    vocabWrong.value = "";
    vocabRight.value = "";
    vocabWrong.focus();
  } catch (err) {
    promptStatus.textContent = `${err}`;
    promptStatus.style.color = "var(--signal)";
  }
});

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

function syncModels() {
  // Model dropdown exists only for the multimodal / hybrid providers.
  const hasModelSelect = ["gemini", "qwen", "mimo"].includes(currentProvider);
  $("model-field").hidden = !hasModelSelect;

  const groups: Record<string, HTMLOptGroupElement> = {
    gemini: $("gemini-models") as unknown as HTMLOptGroupElement,
    qwen: $("qwen-models") as unknown as HTMLOptGroupElement,
    mimo: $("mimo-models") as unknown as HTMLOptGroupElement,
  };
  for (const [key, group] of Object.entries(groups)) {
    group.style.display = key === currentProvider ? "" : "none";
  }

  if (hasModelSelect) {
    const prefixes: Record<string, string> = {
      gemini: "gemini",
      qwen: "qwen",
      mimo: "mimo",
    };
    const defaults: Record<string, string> = {
      gemini: "gemini-3-flash-preview",
      qwen: "qwen3.5-omni-flash",
      mimo: "mimo-v2.5-asr",
    };
    if (!modelEl.value.startsWith(prefixes[currentProvider])) {
      modelEl.value = defaults[currentProvider];
    }
  }

  $("gemini-key-field").style.display = currentProvider === "gemini" ? "" : "none";
  $("qwen-fields").hidden = currentProvider !== "qwen";
  $("mimo-fields").hidden = currentProvider !== "mimo";
  $("volc-fields").hidden = currentProvider !== "volcengine";
  $("whisper-fields").hidden = currentProvider !== "whisper";
  $("apple-fields").hidden = currentProvider !== "apple";
}

function syncCustomLlmFields() {
  customLlmFields.hidden = postprocessProviderEl.value !== "custom";
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
postprocessProviderEl.addEventListener("change", syncCustomLlmFields);
soundFeedbackEl.addEventListener("change", () => {
  soundEnabled = soundFeedbackEl.checked;
});

document.querySelectorAll(".toggle-key-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    const input = $<HTMLInputElement>((btn as HTMLElement).dataset.target!);
    input.type = input.type === "password" ? "text" : "password";
  });
});

// ---- Hotkey capture ----

const CAPTURE_PLACEHOLDER = "按下组合键…";

function attachHotkeyCapture(
  input: HTMLInputElement,
  opts: { fallback: string; clearable?: boolean }
) {
  input.addEventListener("beforeinput", (e) => e.preventDefault());
  input.addEventListener("paste", (e) => e.preventDefault());
  input.addEventListener("focus", () => {
    input.value = CAPTURE_PLACEHOLDER;
  });

  input.addEventListener("keydown", (e) => {
    e.preventDefault();
    e.stopPropagation();

    // 可清空的快捷键：单按 ⌫/Delete 置空（= 禁用）
    if (
      opts.clearable &&
      (e.key === "Backspace" || e.key === "Delete") &&
      !e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey
    ) {
      input.value = "";
      input.dataset.prev = "";
      input.blur();
      return;
    }

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

    input.value = parts.join("+");
    input.dataset.prev = input.value;
    input.blur();
  });

  input.addEventListener("blur", () => {
    if (input.value === CAPTURE_PLACEHOLDER) {
      input.value = input.dataset.prev ?? opts.fallback;
    } else {
      input.dataset.prev = input.value;
    }
  });
}

attachHotkeyCapture(hotkeyEl, { fallback: "Ctrl+." });
attachHotkeyCapture(editHotkeyEl, { fallback: "", clearable: true });

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
        qwen_base_url: qwenBaseUrlEl.value,
        mimo_api_key: mimoKeyEl.value,
        mimo_base_url: mimoBaseUrlEl.value,
        volc_app_key: volcAppKeyEl.value,
        volc_access_key: volcAccessKeyEl.value,
        volc_resource_id: volcResourceIdEl.value,
        whisper_base_url: whisperBaseUrlEl.value,
        whisper_api_key: whisperApiKeyEl.value,
        whisper_model: whisperModelEl.value,
        apple_locale: appleLocaleEl.value,
        enable_postprocess: postprocessEnabledEl.checked,
        postprocess_provider: postprocessProviderEl.value,
        custom_llm_base_url: customLlmBaseUrlEl.value,
        custom_llm_api_key: customLlmApiKeyEl.value,
        custom_llm_model: customLlmModelEl.value,
        model_name: modelEl.value,
        hotkey: hotkeyEl.value,
        edit_hotkey: editHotkeyEl.value,
        audio_device: audioDeviceEl.value,
        input_mode: inputModeEl.value,
        auto_copy: autoCopyEl.checked,
        output_style: currentStyle,
        enable_app_context: appContextEl.checked,
        structured_output: structuredOutputEl.checked,
        stream_typing: streamTypingEl.checked,
        sound_feedback: soundFeedbackEl.checked,
        auto_enter: autoEnterEl.checked,
        app_rules: appRulesEl.value,
        onboarded: true,
        has_gemini_key: true,
        has_qwen_key: true,
        has_mimo_key: true,
        has_volc_keys: true,
        has_whisper_key: true,
        has_custom_llm_key: true,
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

    updateHotkeyHints();

    if (geminiKeyEl.value) {
      geminiKeyEl.value = "";
      setKeyHint(geminiKeyEl, geminiKeyHint, true);
    }
    if (qwenKeyEl.value) {
      qwenKeyEl.value = "";
      setKeyHint(qwenKeyEl, qwenKeyHint, true);
    }
    if (mimoKeyEl.value) {
      mimoKeyEl.value = "";
      setKeyHint(mimoKeyEl, mimoKeyHint, true);
    }
    if (volcAppKeyEl.value) {
      volcAppKeyEl.value = "";
      setKeyHint(volcAppKeyEl, volcAppKeyHint, true);
    }
    if (volcAccessKeyEl.value) {
      volcAccessKeyEl.value = "";
      setKeyHint(volcAccessKeyEl, volcAccessKeyHint, true);
    }
    if (whisperApiKeyEl.value) {
      whisperApiKeyEl.value = "";
      setKeyHint(whisperApiKeyEl, whisperKeyHint, true, true);
    }
    if (customLlmApiKeyEl.value) {
      customLlmApiKeyEl.value = "";
      setKeyHint(customLlmApiKeyEl, customLlmKeyHint, true);
    }
  } catch (err) {
    saveStatus.textContent = `${err}`;
    saveStatus.className = "s-msg bad";
  }
});

// ============ 首次启动引导 ============

const onboardEl = $("onboard");
let obStep = 1;

function showObStep(n: number) {
  obStep = n;
  onboardEl.querySelectorAll<HTMLElement>(".ob-step").forEach((el) => {
    el.hidden = Number(el.dataset.step) !== n;
  });
  $("ob-dots")
    .querySelectorAll("i")
    .forEach((dot, i) => dot.classList.toggle("on", i === n - 1));
  if (n === 3) void refreshObPermission();
}

function startOnboarding() {
  onboardEl.hidden = false;
  // Apple 本地识别只在 macOS 有意义
  if (!navigator.platform.toLowerCase().includes("mac")) {
    $("ob-apple-option").hidden = true;
  }
  showObStep(1);
}

async function refreshObPermission() {
  const statusEl = $("ob-ax-status");
  try {
    const perms = await invoke<Permissions>("check_permissions");
    statusEl.textContent = perms.accessibility ? "已授权 ✓" : "未授权";
    statusEl.className = perms.accessibility ? "s-hint ok" : "s-hint bad";
    $("ob-ax-open").hidden = perms.accessibility;
  } catch {
    statusEl.textContent = "无法检查";
  }
}

onboardEl.querySelectorAll<HTMLButtonElement>(".ob-next").forEach((btn) => {
  btn.addEventListener("click", () => showObStep(obStep + 1));
});

onboardEl.querySelectorAll<HTMLButtonElement>(".ob-skip").forEach((btn) => {
  btn.addEventListener("click", () => void finishOnboarding());
});

// 引擎选择：切到 Apple 时隐藏 Key 输入
onboardEl.querySelectorAll<HTMLInputElement>('input[name="ob-engine"]').forEach((radio) => {
  radio.addEventListener("change", () => {
    $("ob-key").hidden = radio.value === "apple" && radio.checked;
  });
});

$("ob-engine-next").addEventListener("click", async () => {
  const engine =
    onboardEl.querySelector<HTMLInputElement>('input[name="ob-engine"]:checked')?.value ?? "qwen";
  const key = $<HTMLInputElement>("ob-key").value.trim();
  const errEl = $("ob-engine-err");
  errEl.textContent = "";

  if (engine === "qwen" && !key) {
    errEl.textContent = "请粘贴 API Key，或选择 Apple 本地识别 / 先跳过";
    return;
  }
  try {
    await invoke("quick_setup", { provider: engine, apiKey: key });
    setProvider(engine);
    setupNudge.hidden = true;
    if (engine === "qwen" && key) setKeyHint(qwenKeyEl, qwenKeyHint, true);
    showObStep(3);
  } catch (err) {
    errEl.textContent = `${err}`;
  }
});

$("ob-ax-open").addEventListener("click", () => {
  void invoke("open_accessibility_settings");
});

window.addEventListener("focus", () => {
  if (!onboardEl.hidden && obStep === 3) void refreshObPermission();
});

// 试听写：点按开始/结束，结果由 notype://result 事件回填
$("ob-rec").addEventListener("click", async () => {
  try {
    await invoke<boolean>("toggle_recording");
  } catch (err) {
    const r = $("ob-result");
    r.hidden = false;
    r.textContent = `${err}`;
  }
});

$("ob-finish").addEventListener("click", () => void finishOnboarding());

async function finishOnboarding() {
  try {
    await invoke("mark_onboarded");
  } catch (err) {
    console.error("mark_onboarded failed:", err);
  }
  onboardEl.hidden = true;
}

// ============ Init ============

async function init() {
  const version: string = await invoke("get_version");
  versionEl.textContent = `v${version}`;

  const config: Config = await invoke("get_config");
  setProvider(config.provider);
  modelEl.value = config.model_name;
  mimoBaseUrlEl.value = config.mimo_base_url || "https://api.xiaomimimo.com/v1";
  qwenBaseUrlEl.value = config.qwen_base_url || "https://dashscope.aliyuncs.com/compatible-mode/v1";
  volcResourceIdEl.value = config.volc_resource_id || "volc.bigasr.sauc.duration";
  whisperBaseUrlEl.value = config.whisper_base_url || "https://api.openai.com/v1";
  whisperModelEl.value = config.whisper_model || "whisper-1";
  appleLocaleEl.value = config.apple_locale || "";
  postprocessEnabledEl.checked = config.enable_postprocess;
  structuredOutputEl.checked = config.structured_output;
  postprocessProviderEl.value = config.postprocess_provider || "auto";
  customLlmBaseUrlEl.value = config.custom_llm_base_url || "";
  customLlmModelEl.value = config.custom_llm_model || "";
  streamTypingEl.checked = config.stream_typing;
  soundFeedbackEl.checked = config.sound_feedback;
  soundEnabled = config.sound_feedback;
  autoEnterEl.checked = config.auto_enter;
  appRulesEl.value = config.app_rules || "";
  hotkeyEl.value = config.hotkey;
  hotkeyEl.dataset.prev = config.hotkey;
  editHotkeyEl.value = config.edit_hotkey ?? "";
  editHotkeyEl.dataset.prev = editHotkeyEl.value;
  inputModeEl.value = config.input_mode === "clipboard" ? "clipboard" : "keyboard";
  autoCopyEl.checked = config.auto_copy;
  setStyleSeg(config.output_style || "polish");
  appContextEl.checked = config.enable_app_context;

  updateHotkeyHints();

  setKeyHint(geminiKeyEl, geminiKeyHint, config.has_gemini_key);
  setKeyHint(qwenKeyEl, qwenKeyHint, config.has_qwen_key);
  setKeyHint(mimoKeyEl, mimoKeyHint, config.has_mimo_key);
  setKeyHint(volcAppKeyEl, volcAppKeyHint, config.has_volc_keys);
  setKeyHint(volcAccessKeyEl, volcAccessKeyHint, config.has_volc_keys);
  setKeyHint(whisperApiKeyEl, whisperKeyHint, config.has_whisper_key, true);
  setKeyHint(customLlmApiKeyEl, customLlmKeyHint, config.has_custom_llm_key);

  syncModels();
  syncCustomLlmFields();

  // Prompts
  currentPrompts = await invoke("get_prompts");
  promptEditor.value = currentPrompts[activeTab];
  builtinPromptsCache = await invoke("get_builtin_prompts");

  // 当前服务商缺少凭证时直接进设置页
  const qwenIsLocal =
    !!config.qwen_base_url &&
    !config.qwen_base_url.includes("dashscope.aliyuncs.com");
  const needsKey =
    (config.provider === "gemini" && !config.has_gemini_key) ||
    (config.provider === "qwen" && !config.has_qwen_key && !qwenIsLocal) ||
    (config.provider === "mimo" && !config.has_mimo_key) ||
    (config.provider === "volcengine" && !config.has_volc_keys);
  setupNudge.hidden = !needsKey;
  if (needsKey && config.onboarded) showView("settings");

  // 首次启动：进入引导流程
  if (!config.onboarded) startOnboarding();

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

  // 权限状态（macOS 辅助功能）
  await refreshPermissions();

  // 终身统计
  try {
    renderStats(await invoke<Stats>("get_stats"));
  } catch {
    statsStrip.hidden = true;
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
    if (!onboardEl.hidden && obStep === 4) {
      const r = $("ob-result");
      r.hidden = false;
      r.textContent = entry.text;
    }
    showResultCard(entry, "听写结果");
    if (historyLoaded) {
      historyCache.unshift(entry);
      renderHistory();
    }
  });

  await listen<number>("notype://level", (e) => {
    driveWaveform(e.payload);
  });

  await listen<Stats>("notype://stats", (e) => {
    renderStats(e.payload);
  });
}

function updateHotkeyHints() {
  $("hotkey-hint").textContent = hotkeyEl.value.replace(/\+/g, " + ");
  const editHint = $("edit-hint");
  if (editHotkeyEl.value.trim()) {
    editHint.hidden = false;
    $("edit-hotkey-hint").textContent = editHotkeyEl.value.replace(/\+/g, " + ");
  } else {
    editHint.hidden = true;
  }
}

async function refreshPermissions() {
  try {
    const perms = await invoke<Permissions>("check_permissions");
    permRow.hidden = perms.accessibility;
  } catch {
    permRow.hidden = true;
  }
}

$("open-accessibility").addEventListener("click", () => {
  void invoke("open_accessibility_settings");
});

// 用户去系统设置授权后切回来时重新检查
window.addEventListener("focus", () => {
  if (!permRow.hidden) void refreshPermissions();
});

window.addEventListener("DOMContentLoaded", () => {
  void init();
});
