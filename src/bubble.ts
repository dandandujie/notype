import { prepareWithSegments, layoutWithLines } from "@chenglou/pretext";

// Extend Window for global functions called from Rust eval()
declare global {
  interface Window {
    showRecording: () => void;
    showRecognizing: () => void;
    showInterim: (text: string) => void;
    showResult: (text: string) => void;
    showError: (text: string) => void;
  }
}

// -- Constants --
const BUBBLE_WIDTH = 380;
const CARD_PAD_V = 32; // card vertical padding (16*2)
const CONTAINER_PAD = 26; // container padding top+bottom
const PILL_SECTION = 62; // pill height(48) + gap(14)
const BUBBLE_WINDOW_H = 210;
const MIN_CARD_H = 48;
const DPR = window.devicePixelRatio || 2;

const FONT_SIZE = 14.5;
const FONT_FAMILY =
  "-apple-system, BlinkMacSystemFont, SF Pro Text, Helvetica Neue, sans-serif";
const FONT = `${FONT_SIZE}px ${FONT_FAMILY}`;
const LINE_HEIGHT = Math.round(FONT_SIZE * 1.65);
const TEXT_WIDTH = BUBBLE_WIDTH - 72; // card padding(40) + container horiz padding(32)

// Animation timing
const CHAR_STAGGER_MS = 25; // delay between each character's fade start
const CHAR_FADE_DURATION_MS = 300; // how long each character takes to fully appear

// -- State --
let prevText = "";
let currentText = "";
let animChars: CharAnim[] = [];
let animRafId = 0;
let isInterim = false;
let cursorVisible = false;
let cursorBlinkId = 0;

interface CharAnim {
  char: string;
  x: number;
  y: number;
  startTime: number;
  settled: boolean; // already fully visible from a previous render
}

// -- DOM refs --
const $ = (id: string) => document.getElementById(id)!;

function getCanvas(): HTMLCanvasElement {
  return $("text-canvas") as HTMLCanvasElement;
}

function getCtx(): CanvasRenderingContext2D {
  return getCanvas().getContext("2d")!;
}

function isDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function textColor(): string {
  return isDark() ? "#f5f5f7" : "#1d1d1f";
}

// -- Pretext measurement & line layout --
interface LayoutResult {
  lines: { text: string }[];
  height: number;
  charPositions: { char: string; x: number; y: number }[];
}

function measureAndLayout(text: string): LayoutResult {
  if (!text) {
    return { lines: [], height: 0, charPositions: [] };
  }

  const prepared = prepareWithSegments(text, FONT, { whiteSpace: "pre-wrap" });
  const result = layoutWithLines(prepared, TEXT_WIDTH, LINE_HEIGHT);

  // Build per-character positions from lines
  const ctx = getCtx();
  ctx.font = FONT;
  const charPositions: { char: string; x: number; y: number }[] = [];

  for (let li = 0; li < result.lines.length; li++) {
    const lineText = result.lines[li].text;
    const baseY = li * LINE_HEIGHT + FONT_SIZE; // baseline
    let x = 0;

    for (const ch of lineText) {
      charPositions.push({ char: ch, x, y: baseY });
      x += ctx.measureText(ch).width;
    }
  }

  return {
    lines: result.lines.map((l) => ({ text: l.text })),
    height: result.height,
    charPositions,
  };
}

// -- Canvas sizing --
function sizeCanvas(height: number) {
  const canvas = getCanvas();
  const cssW = TEXT_WIDTH;
  const cssH = Math.max(LINE_HEIGHT, height);
  canvas.style.width = `${cssW}px`;
  canvas.style.height = `${cssH}px`;
  canvas.width = Math.round(cssW * DPR);
  canvas.height = Math.round(cssH * DPR);
  const ctx = getCtx();
  ctx.scale(DPR, DPR);
}

function resizeBubble(textHeight: number, showPill: boolean, autoScrollBottom = false) {
  const card = $("card");
  const cardContent = $("card-content");
  const cardH = Math.max(
    MIN_CARD_H,
    BUBBLE_WINDOW_H - CONTAINER_PAD - (showPill ? PILL_SECTION : 0)
  );
  const viewportH = Math.max(LINE_HEIGHT, cardH - CARD_PAD_V);

  card.style.height = `${cardH}px`;
  cardContent.style.maxHeight = `${viewportH}px`;
  cardContent.style.overflowX = "hidden";
  cardContent.style.overflowY = textHeight > viewportH ? "auto" : "hidden";

  if (autoScrollBottom) {
    scrollCardToBottom(cardContent);
  } else if (textHeight <= viewportH) {
    cardContent.scrollTop = 0;
  }
}

function scrollCardToBottom(card: HTMLElement) {
  const apply = () => {
    card.scrollTop = Math.max(0, card.scrollHeight - card.clientHeight);
  };
  apply();
  requestAnimationFrame(() => {
    apply();
    requestAnimationFrame(apply);
  });
  window.setTimeout(apply, 0);
  window.setTimeout(apply, 16);
  window.setTimeout(apply, 64);
  window.setTimeout(apply, 160);
}

function collapseImmediateRepeatedTail(text: string): string {
  const chars = [...text];
  const total = chars.length;
  if (total < 24) return text;

  const maxBlock = Math.min(120, Math.floor(total / 2));
  for (let block = maxBlock; block >= 12; block--) {
    const left = chars.slice(total - block * 2, total - block).join("");
    const right = chars.slice(total - block).join("");
    if (left === right) {
      return chars.slice(0, total - block).join("");
    }
    const leftNorm = normalizeSentence(left);
    const rightNorm = normalizeSentence(right);
    if (leftNorm && rightNorm && sameOrSimilarNorm(leftNorm, rightNorm)) {
      return chars.slice(0, total - block * 2).join("") + right.trimStart();
    }
  }
  return text;
}

function isSentenceBoundary(ch: string): boolean {
  return (
    ch === "。" ||
    ch === "！" ||
    ch === "？" ||
    ch === "；" ||
    ch === "，" ||
    ch === "、" ||
    ch === "," ||
    ch === ":" ||
    ch === "：" ||
    ch === "." ||
    ch === "!" ||
    ch === "?" ||
    ch === ";" ||
    ch === "\n"
  );
}

function normalizeSentence(sentence: string): string {
  return [...sentence]
    .filter((ch) => /[\p{Letter}\p{Number}]/u.test(ch))
    .join("")
    .toLowerCase();
}

function commonPrefixChars(left: string, right: string): number {
  const leftChars = [...left];
  const rightChars = [...right];
  let count = 0;
  while (count < leftChars.length && count < rightChars.length && leftChars[count] === rightChars[count]) {
    count += 1;
  }
  return count;
}

function levenshteinDistance(left: string, right: string): number {
  const a = [...left];
  const b = [...right];
  if (!a.length) return b.length;
  if (!b.length) return a.length;

  let prev = Array.from({ length: b.length + 1 }, (_, idx) => idx);
  let curr = new Array<number>(b.length + 1).fill(0);

  for (let i = 0; i < a.length; i += 1) {
    curr[0] = i + 1;
    for (let j = 0; j < b.length; j += 1) {
      const cost = a[i] === b[j] ? 0 : 1;
      curr[j + 1] = Math.min(prev[j + 1] + 1, curr[j] + 1, prev[j] + cost);
    }
    [prev, curr] = [curr, prev];
  }
  return prev[b.length];
}

function similarSentenceScore(leftNorm: string, rightNorm: string): number {
  if (!leftNorm || !rightNorm) return 0;
  if (leftNorm === rightNorm) return 1;
  const maxLen = Math.max([...leftNorm].length, [...rightNorm].length);
  if (!maxLen) return 0;
  if (leftNorm.includes(rightNorm) || rightNorm.includes(leftNorm)) {
    return Math.min([...leftNorm].length, [...rightNorm].length) / maxLen;
  }
  const prefixRatio = commonPrefixChars(leftNorm, rightNorm) / maxLen;
  const editRatio = 1 - levenshteinDistance(leftNorm, rightNorm) / maxLen;
  return Math.max(prefixRatio, editRatio);
}

function sameOrSimilarNorm(leftNorm: string, rightNorm: string): boolean {
  return (
    leftNorm === rightNorm ||
    leftNorm.startsWith(rightNorm) ||
    rightNorm.startsWith(leftNorm) ||
    (Math.min([...leftNorm].length, [...rightNorm].length) >= 8 &&
      similarSentenceScore(leftNorm, rightNorm) >= 0.82)
  );
}

function splitSentenceSegments(text: string): string[] {
  const segments: string[] = [];
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (!isSentenceBoundary(ch)) continue;
    const segment = text.slice(start, i + 1).trim();
    if (segment) segments.push(segment);
    start = i + 1;
  }
  const tail = text.slice(start).trim();
  if (tail) segments.push(tail);
  return segments;
}

function compactRepeatedSentences(text: string): string {
  let working = text.trim();
  for (let i = 0; i < 4; i += 1) {
    const collapsed = collapseImmediateRepeatedTail(working);
    if (collapsed === working) break;
    working = collapsed;
  }

  const kept: string[] = [];
  for (const segment of splitSentenceSegments(working)) {
    const norm = normalizeSentence(segment);
    if (!norm) {
      kept.push(segment);
      continue;
    }

    let handled = false;
    const start = Math.max(0, kept.length - 12);
    for (let idx = kept.length - 1; idx >= start; idx -= 1) {
      const prevNorm = normalizeSentence(kept[idx]);
      if (!prevNorm) continue;
      if (norm === prevNorm) {
        handled = true;
        break;
      }
      if (sameOrSimilarNorm(norm, prevNorm)) {
        if (norm.length >= prevNorm.length) {
          kept[idx] = segment;
        }
        handled = true;
        break;
      }
      if (norm.startsWith(prevNorm)) {
        kept[idx] = segment;
        handled = true;
        break;
      }
      if (prevNorm.startsWith(norm)) {
        handled = true;
        break;
      }
    }

    if (!handled) kept.push(segment);
  }
  return collapseImmediateRepeatedTail(kept.join(""));
}

function commonPrefixCodeUnitCount(a: string, b: string): number {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i += 1;
  return i;
}

// -- Animation engine --
function buildAnimChars(
  layout: LayoutResult,
  prevCharCount: number
): CharAnim[] {
  const now = performance.now();
  return layout.charPositions.map((cp, i) => ({
    char: cp.char,
    x: cp.x,
    y: cp.y,
    startTime: i < prevCharCount ? 0 : now + (i - prevCharCount) * CHAR_STAGGER_MS,
    settled: i < prevCharCount, // chars from previous text are already visible
  }));
}

function renderFrame() {
  const canvas = getCanvas();
  const ctx = canvas.getContext("2d")!;
  ctx.clearRect(0, 0, canvas.width / DPR, canvas.height / DPR);
  ctx.font = FONT;
  ctx.textBaseline = "alphabetic";

  const now = performance.now();
  let allSettled = true;

  for (const ac of animChars) {
    if (ac.settled) {
      ctx.globalAlpha = 1;
    } else {
      const elapsed = now - ac.startTime;
      if (elapsed < 0) {
        allSettled = false;
        continue; // not started yet
      }
      const progress = Math.min(1, elapsed / CHAR_FADE_DURATION_MS);
      // Ease-out cubic
      const eased = 1 - Math.pow(1 - progress, 3);
      ctx.globalAlpha = eased;
      if (progress < 1) allSettled = false;
    }

    ctx.fillStyle = textColor();
    // Subtle vertical offset for unsettled chars (slide up 3px)
    const yOffset = ac.settled ? 0 : (1 - ctx.globalAlpha) * 3;
    ctx.fillText(ac.char, ac.x, ac.y + yOffset);
  }

  // Draw blinking cursor for interim
  if (isInterim && cursorVisible && animChars.length > 0) {
    const last = animChars[animChars.length - 1];
    const cursorX = last.x + ctx.measureText(last.char).width + 2;
    ctx.globalAlpha = 0.4;
    ctx.fillStyle = textColor();
    ctx.fillRect(cursorX, last.y - FONT_SIZE + 2, 2, FONT_SIZE + 2);
  }

  ctx.globalAlpha = 1;

  if (!allSettled) {
    animRafId = requestAnimationFrame(renderFrame);
  } else {
    animRafId = 0;
    // Mark all as settled
    for (const ac of animChars) ac.settled = true;
    // If interim, keep rendering for cursor blink
    if (isInterim) {
      animRafId = requestAnimationFrame(renderFrame);
    }
  }
}

function startAnimation(text: string, prevCount: number, showPill: boolean) {
  const layoutResult = measureAndLayout(text);
  sizeCanvas(layoutResult.height);
  animChars = buildAnimChars(layoutResult, prevCount);

  if (animRafId) cancelAnimationFrame(animRafId);
  animRafId = requestAnimationFrame(renderFrame);

  resizeBubble(layoutResult.height, showPill, showPill);
}

function renderInterimStable(text: string) {
  const layoutResult = measureAndLayout(text);
  sizeCanvas(layoutResult.height);
  animChars = layoutResult.charPositions.map((cp) => ({
    char: cp.char,
    x: cp.x,
    y: cp.y,
    startTime: 0,
    settled: true,
  }));

  if (animRafId) cancelAnimationFrame(animRafId);
  animRafId = requestAnimationFrame(renderFrame);
  resizeBubble(layoutResult.height, true, true);
}

function stabilizeInterimText(previous: string, incoming: string): string {
  const prev = previous.trim();
  const next = compactRepeatedSentences(collapseImmediateRepeatedTail(incoming.trim()));
  if (!prev) return next;
  if (!next) return prev;
  if (next.startsWith(prev)) return next;
  if (prev.startsWith(next)) {
    const prevNorm = normalizeSentence(prev);
    const nextNorm = normalizeSentence(next);
    if (sameOrSimilarNorm(prevNorm, nextNorm)) {
      return next;
    }
    const shrink = prev.length - next.length;
    return shrink > 24 ? prev : next;
  }
  const prefix = commonPrefixCodeUnitCount(prev, next);
  if (next.length + 24 < prev.length && prefix < Math.floor(prev.length * 0.35)) {
    return prev;
  }
  return next;
}

function stopCursorBlink() {
  if (cursorBlinkId) {
    clearInterval(cursorBlinkId);
    cursorBlinkId = 0;
  }
  cursorVisible = false;
}

function startCursorBlink() {
  stopCursorBlink();
  cursorVisible = true;
  cursorBlinkId = window.setInterval(() => {
    cursorVisible = !cursorVisible;
    // renderFrame will pick up the change on next frame
    if (!animRafId) {
      animRafId = requestAnimationFrame(renderFrame);
    }
  }, 600);
}

// Count characters in common prefix
function commonPrefixCharCount(a: string, b: string): number {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  // Count graphemes (spread handles surrogate pairs)
  return [...a.slice(0, i)].length;
}

// -- Copy as image --
function renderToImageCanvas(text: string): HTMLCanvasElement {
  const imgCanvas = document.createElement("canvas");
  const pad = 32;
  const maxW = 600;
  const textW = maxW - pad * 2;

  const prepared = prepareWithSegments(text, FONT, { whiteSpace: "pre-wrap" });
  const result = layoutWithLines(prepared, textW, LINE_HEIGHT);

  const totalH = result.height + pad * 2;
  imgCanvas.width = Math.round(maxW * DPR);
  imgCanvas.height = Math.round(totalH * DPR);

  const ctx = imgCanvas.getContext("2d")!;
  ctx.scale(DPR, DPR);

  // Background
  ctx.fillStyle = isDark() ? "#2c2c2e" : "#ffffff";
  const r = 16;
  ctx.beginPath();
  ctx.moveTo(r, 0);
  ctx.lineTo(maxW - r, 0);
  ctx.quadraticCurveTo(maxW, 0, maxW, r);
  ctx.lineTo(maxW, totalH - r);
  ctx.quadraticCurveTo(maxW, totalH, maxW - r, totalH);
  ctx.lineTo(r, totalH);
  ctx.quadraticCurveTo(0, totalH, 0, totalH - r);
  ctx.lineTo(0, r);
  ctx.quadraticCurveTo(0, 0, r, 0);
  ctx.closePath();
  ctx.fill();

  // Text
  ctx.font = FONT;
  ctx.fillStyle = textColor();
  ctx.textBaseline = "alphabetic";

  for (let li = 0; li < result.lines.length; li++) {
    ctx.fillText(result.lines[li].text, pad, pad + li * LINE_HEIGHT + FONT_SIZE);
  }

  return imgCanvas;
}

async function copyAsImage() {
  if (!currentText) return;

  const imgCanvas = renderToImageCanvas(currentText);

  try {
    const blob = await new Promise<Blob | null>((resolve) =>
      imgCanvas.toBlob(resolve, "image/png")
    );
    if (blob) {
      await navigator.clipboard.write([
        new ClipboardItem({ "image/png": blob }),
      ]);
      // Flash feedback on button
      const btn = $("copy-btn");
      btn.style.opacity = "1";
      btn.title = "Copied!";
      setTimeout(() => {
        btn.style.opacity = "";
        btn.title = "Copy as image";
      }, 1500);
    }
  } catch (e) {
    console.error("Failed to copy image:", e);
  }
}

// Set up copy button
$("copy-btn").addEventListener("click", copyAsImage);

// -- Public API --

window.showRecording = function () {
  prevText = "";
  currentText = "";
  isInterim = false;
  stopCursorBlink();
  animChars = [];
  if (animRafId) {
    cancelAnimationFrame(animRafId);
    animRafId = 0;
  }

  $("dots").className = "typing-dots";
  getCanvas().className = "hidden";
  $("copy-btn").classList.remove("visible");
  $("pill").className = "pill recording";
  $("card").className = "card";
  $("card-content").scrollTop = 0;

  resizeBubble(0, true, false);
};

window.showRecognizing = function () {
  isInterim = false;
  stopCursorBlink();
  $("pill").className = "pill recognizing";
};

window.showInterim = function (text: string) {
  $("dots").className = "typing-dots hidden";
  getCanvas().className = "";
  $("copy-btn").classList.remove("visible");
  $("card").className = "card text-mode";

  isInterim = true;
  startCursorBlink();

  const stabilized = stabilizeInterimText(currentText, text);
  const prevCount = commonPrefixCharCount(prevText, stabilized);
  if (stabilized === currentText && prevCount === [...stabilized].length) {
    return;
  }
  prevText = stabilized;
  currentText = stabilized;

  // Interim updates are high-frequency; avoid restart animations to prevent visual flicker.
  renderInterimStable(stabilized);
};

window.showResult = function (text: string) {
  $("dots").className = "typing-dots hidden";
  getCanvas().className = "";
  $("pill").className = "pill ready";
  $("card").className = "card text-mode";

  isInterim = false;
  stopCursorBlink();

  const prevCount = commonPrefixCharCount(prevText, text);
  prevText = text;
  currentText = text;

  startAnimation(text, prevCount, true);
  requestAnimationFrame(() => {
    scrollCardToBottom($("card-content"));
  });

  // Show copy button after result
  $("copy-btn").classList.add("visible");
};

window.showError = function (text: string) {
  $("dots").className = "typing-dots hidden";
  getCanvas().className = "";
  $("pill").className = "pill hidden";
  $("card").className = "card text-mode";

  isInterim = false;
  stopCursorBlink();
  prevText = "";
  currentText = text;

  startAnimation(text, 0, false);
  $("copy-btn").classList.remove("visible");
};
