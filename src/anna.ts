import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, LogicalPosition } from "@tauri-apps/api/window";

interface AnnaState {
  prompt: string;
  response: string | null;
  is_error: boolean;
}

const PANEL_W = 380;
const RIGHT_MARGIN = 20;
const TOP_MARGIN = 60; // clears macOS menu bar (~28 px) + breathing room
const SLIDE_MS = 300;

const win = getCurrentWindow();
const card = document.getElementById("card")!;
const thinkingEl = document.getElementById("thinking")!;
const responseText = document.getElementById("response-text")!;
const copyBtn = document.getElementById("copy-btn")!;
const closeBtn = document.getElementById("close-btn")!;
let isOpen = false;

// ── State helpers ──

function showThinking() {
  thinkingEl.classList.remove("hidden");
  responseText.classList.remove("visible", "error");
}

function showResponse(response: string, isError: boolean) {
  thinkingEl.classList.add("hidden");
  responseText.textContent = response;
  responseText.classList.toggle("error", isError);
  responseText.classList.add("visible");
}

// ── Open ──

async function openPanel() {
  isOpen = true;

  // 1. Position window before making it visible — avoids flash at wrong location
  const x = window.screen.width - PANEL_W - RIGHT_MARGIN;
  await win.setPosition(new LogicalPosition(x, TOP_MARGIN));
  await win.show();
  await win.setFocus();

  // 2. Read state (handles race where response arrived before window loaded)
  const state = await invoke<AnnaState | null>("get_anna_state");
  if (state) {
    if (state.response !== null) {
      showResponse(state.response, state.is_error);
    } else {
      showThinking();
    }
  }

  // 3. Trigger slide-in on next frame so the browser has painted the positioned window
  requestAnimationFrame(() => card.classList.add("visible"));
}

// ── Close ──

async function closePanel() {
  isOpen = false;
  card.classList.remove("visible");
  await new Promise((r) => setTimeout(r, SLIDE_MS));
  await win.hide();
  // Snap card back to start position silently (no transition) for next open
  card.style.transition = "none";
  card.getBoundingClientRect(); // force reflow
  card.style.transition = "";
  showThinking();
  responseText.textContent = "";
}

// ── Events ──

// anna-show: Rust signals that a new query started; JS shows the panel
listen("anna-show", () => openPanel());

// anna-thinking: already-open panel resets to thinking state
listen<{ prompt: string }>("anna-thinking", () => showThinking());

// anna-response: response ready
listen<{ prompt: string; response: string; error: boolean }>("anna-response", (e) => {
  showResponse(e.payload.response, e.payload.error);
});

invoke<AnnaState | null>("get_anna_state")
  .then((state) => {
    if (state && !isOpen) {
      openPanel();
    }
  })
  .catch(() => {});

// ── Actions ──

copyBtn.addEventListener("click", () => {
  const text = responseText.textContent;
  if (text) navigator.clipboard.writeText(text);
});

closeBtn.addEventListener("click", closePanel);

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closePanel();
});
