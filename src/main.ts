import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Timestamp } from "firebase/firestore";
import { initAuth, signIn } from "./auth";
import {
  RecordingMeta,
  subscribeToRecordings,
  saveRecording,
  updateRecording,
  deleteRecordingDoc,
  subscribeToControl,
  resetControl,
} from "./db";

interface StopRecordingResult {
  id: string;
  duration_secs: number;
  has_system_audio: boolean;
}

interface DownloadProgress {
  downloaded: number;
  total: number | null;
  percent: number;
}

type TranscriptionProvider = "local" | "openai";

interface AppSettings {
  openai_api_key: string | null;
  transcription_provider: TranscriptionProvider;
}

let currentUid: string | null = null;
let isRecording = false;
let recordingStartTime: number | null = null;
let timerInterval: number | null = null;
let selectedRecordingId: string | null = null;
let recordings: RecordingMeta[] = [];
let currentRecordingId: string | null = null;
let liveTranscriptionText = "";
let unsubRecordings: (() => void) | null = null;
let unsubControl: (() => void) | null = null;

function cleanupSubscriptions() {
  if (unsubRecordings) { unsubRecordings(); unsubRecordings = null; }
  if (unsubControl) { unsubControl(); unsubControl = null; }
}

// ─── DOM Elements ───

const authOverlay = document.getElementById("auth-overlay")!;
const loginBtn = document.getElementById("login-btn") as HTMLButtonElement;
const authError = document.getElementById("auth-error")!;
const recordBtn = document.getElementById("record-btn") as HTMLButtonElement;
const recordingIndicator = document.getElementById("recording-indicator")!;
const recordingTimer = document.getElementById("recording-timer")!;
const recordingsList = document.getElementById("recordings-list")!;
const detailEmpty = document.getElementById("detail-empty")!;
const detailView = document.getElementById("detail-view")!;
const detailTitle = document.getElementById("detail-title")!;
const detailDate = document.getElementById("detail-date")!;
const detailDuration = document.getElementById("detail-duration")!;
const audioPlayer = document.getElementById("audio-player") as HTMLAudioElement;
const transcriptionText = document.getElementById("transcription-text")!;
const transcriptionStatus = document.getElementById("transcription-status")!;
const copyBtn = document.getElementById("copy-btn") as HTMLButtonElement;
const retranscribeBtn = document.getElementById(
  "retranscribe-btn"
) as HTMLButtonElement;
const detailTitleInput = document.getElementById(
  "detail-title-input"
) as HTMLInputElement;
const deleteBtn = document.getElementById("delete-btn") as HTMLButtonElement;
const settingsBtn = document.getElementById("settings-btn") as HTMLButtonElement;
const settingsOverlay = document.getElementById("settings-overlay")!;
const openaiKeyInput = document.getElementById("openai-key-input") as HTMLInputElement;
const transcriptionProviderInputs = Array.from(
  document.querySelectorAll<HTMLInputElement>('input[name="transcription-provider"]')
);
const settingsSaveBtn = document.getElementById("settings-save-btn") as HTMLButtonElement;
const settingsCancelBtn = document.getElementById("settings-cancel-btn") as HTMLButtonElement;
const logBtn = document.getElementById("log-btn") as HTMLButtonElement;
const logPanel = document.getElementById("log-panel")!;
const logCloseBtn = document.getElementById(
  "log-close-btn"
) as HTMLButtonElement;
const logContent = document.getElementById("log-content")!;
const modelOverlay = document.getElementById("model-overlay")!;
const downloadBtn = document.getElementById("download-btn") as HTMLButtonElement;
const skipDownloadBtn = document.getElementById(
  "skip-download-btn"
) as HTMLButtonElement;
const downloadProgress = document.getElementById("download-progress")!;
const progressFill = document.getElementById("progress-fill")!;
const progressText = document.getElementById("progress-text")!;

// ─── Error Display ───

function showError(message: string) {
  const existing = document.getElementById("error-toast");
  if (existing) existing.remove();

  const toast = document.createElement("div");
  toast.id = "error-toast";
  toast.className = "error-toast";
  toast.textContent = message;
  document.body.appendChild(toast);
  setTimeout(() => toast.remove(), 6000);
}

function showWarning(message: string) {
  const existing = document.getElementById("warning-toast");
  if (existing) existing.remove();

  const toast = document.createElement("div");
  toast.id = "warning-toast";
  toast.className = "warning-toast";
  toast.textContent = message;
  document.body.appendChild(toast);
  setTimeout(() => toast.remove(), 8000);
}

function setTranscriptionProvider(provider: TranscriptionProvider) {
  for (const input of transcriptionProviderInputs) {
    input.checked = input.value === provider;
  }
}

function getTranscriptionProvider(): TranscriptionProvider {
  const selected = transcriptionProviderInputs.find((input) => input.checked);
  return selected?.value === "openai" ? "openai" : "local";
}

// ─── Transcription Formatting ───

function formatTranscription(text: string): string {
  return text
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => {
      const trimmed = line.trim();
      if (
        trimmed.endsWith(":") &&
        (trimmed === "Sælger:" || trimmed === "Lead:")
      ) {
        const cls = trimmed === "Sælger:" ? "speaker-seller" : "speaker-lead";
        return `<div class="speaker-label ${cls}">${trimmed}</div>`;
      }
      return `<p>${trimmed}</p>`;
    })
    .join("");
}

// ─── Log Panel ───

async function toggleLogPanel() {
  if (logPanel.classList.contains("hidden")) {
    const logs: string[] = await invoke("get_logs");
    logContent.textContent = logs.join("\n");
    logPanel.classList.remove("hidden");
    logContent.scrollTop = logContent.scrollHeight;
  } else {
    logPanel.classList.add("hidden");
  }
}

// ─── Recording Control ───

async function toggleRecording() {
  if (isRecording) {
    await stopRecording();
  } else {
    await startRecording();
  }
}

async function startRecording() {
  try {
    recordBtn.disabled = true;
    recordBtn.textContent = "Starter...";
    const id: string = await invoke("start_recording");
    isRecording = true;
    currentRecordingId = id;
    liveTranscriptionText = "";
    recordBtn.textContent = "Stop optagelse";
    recordBtn.classList.add("recording");
    recordBtn.disabled = false;
    recordingIndicator.classList.remove("hidden");
    recordingStartTime = Date.now();
    timerInterval = window.setInterval(updateTimer, 1000);

    // Show live transcription area
    detailEmpty.classList.add("hidden");
    detailView.classList.remove("hidden");
    detailTitle.textContent = "Optager...";
    detailDate.textContent = "";
    detailDuration.textContent = "";
    audioPlayer.classList.add("hidden");
    transcriptionStatus.textContent = "Live transskription...";
    transcriptionStatus.classList.remove("hidden");
    transcriptionText.innerHTML =
      '<p class="empty-state">Venter på lyd...</p>';
  } catch (e: any) {
    console.error("Kunne ikke starte optagelse:", e);
    recordBtn.disabled = false;
    recordBtn.textContent = "Start optagelse";
    showError(`Kunne ikke starte optagelse: ${e}`);
  }
}

async function stopRecording() {
  if (!currentUid) return;
  try {
    recordBtn.disabled = true;
    recordBtn.textContent = "Stopper...";
    transcriptionStatus.textContent = "Færdiggør transskription...";
    transcriptionStatus.classList.remove("hidden");

    const result: StopRecordingResult = await invoke("stop_recording");
    isRecording = false;
    const stoppedId = currentRecordingId;
    currentRecordingId = null;
    recordBtn.textContent = "Start optagelse";
    recordBtn.classList.remove("recording");
    recordBtn.disabled = false;
    recordingIndicator.classList.add("hidden");
    if (timerInterval) clearInterval(timerInterval);
    recordingTimer.textContent = "00:00";
    audioPlayer.classList.remove("hidden");

    // Save recording metadata to Firestore
    const now = new Date();
    const title = `Optagelse ${now.toLocaleDateString("da-DK", {
      day: "2-digit",
      month: "2-digit",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    })}`;

    const meta: RecordingMeta = {
      id: result.id,
      title,
      created_at: Timestamp.now(),
      duration_secs: result.duration_secs,
      has_system_audio: result.has_system_audio,
      transcription: liveTranscriptionText || null,
      transcription_status: "NotStarted",
    };
    console.log("Saving to Firestore:", currentUid, meta.id);
    try {
      await saveRecording(currentUid, meta);
      console.log("Firestore save OK");
    } catch (err) {
      console.error("Firestore save FAILED:", err);
      showError(`Firestore fejl: ${err}`);
    }

    if (stoppedId) selectRecording(stoppedId);
  } catch (e: any) {
    console.error("Kunne ikke stoppe optagelse:", e);
    showError(`Kunne ikke stoppe optagelse: ${e?.message || e}`);
  }
}

function updateTimer() {
  if (!recordingStartTime) return;
  const elapsed = Math.floor((Date.now() - recordingStartTime) / 1000);
  const min = Math.floor(elapsed / 60)
    .toString()
    .padStart(2, "0");
  const sec = (elapsed % 60).toString().padStart(2, "0");
  recordingTimer.textContent = `${min}:${sec}`;
}

// ─── Recordings List ───

function renderRecordingsList() {
  if (recordings.length === 0) {
    recordingsList.innerHTML =
      '<p class="empty-state">Ingen optagelser endnu</p>';
    return;
  }

  recordingsList.innerHTML = recordings
    .map((r) => {
      const date = r.created_at.toDate().toLocaleDateString("da-DK", {
        day: "2-digit",
        month: "2-digit",
        year: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
      const duration = formatDuration(r.duration_secs);
      const statusBadge = getStatusBadge(r.transcription_status);
      const selected = r.id === selectedRecordingId ? "selected" : "";

      return `
      <div class="recording-item ${selected}" data-id="${r.id}">
        <div class="recording-info">
          <span class="recording-title">${r.title}</span>
          <span class="recording-meta">${date} · ${duration}</span>
        </div>
        ${statusBadge}
      </div>`;
    })
    .join("");

  recordingsList.querySelectorAll(".recording-item").forEach((el) => {
    el.addEventListener("click", () => {
      const id = (el as HTMLElement).dataset.id!;
      selectRecording(id);
    });
  });
}

function getStatusBadge(status: string): string {
  switch (status) {
    case "Done":
      return '<span class="badge badge-done">Klar</span>';
    case "InProgress":
      return '<span class="badge badge-progress">Transskriberer...</span>';
    case "Failed":
      return '<span class="badge badge-failed">Fejl</span>';
    default:
      return "";
  }
}

function formatDuration(secs: number): string {
  const min = Math.floor(secs / 60);
  const sec = Math.floor(secs % 60);
  return `${min}:${sec.toString().padStart(2, "0")}`;
}

// ─── Detail View ───

async function selectRecording(id: string) {
  selectedRecordingId = id;
  const recording = recordings.find((r) => r.id === id);
  if (!recording) return;

  detailEmpty.classList.add("hidden");
  detailView.classList.remove("hidden");

  detailTitle.textContent = recording.title;
  detailDate.textContent = recording.created_at.toDate().toLocaleDateString(
    "da-DK",
    {
      weekday: "long",
      day: "numeric",
      month: "long",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }
  );
  detailDuration.textContent = formatDuration(recording.duration_secs);

  // Load audio
  const wavPath: string = await invoke("get_recording_wav_path", { id });
  audioPlayer.src = convertFileSrc(wavPath);

  // Transcription
  updateTranscriptionView(recording);

  // Highlight in list
  renderRecordingsList();
}

function updateTranscriptionView(recording: RecordingMeta) {
  transcriptionStatus.classList.add("hidden");

  // Use live text, or the transcription saved in Firestore, as preview
  const previewText = liveTranscriptionText || recording.transcription;

  if (recording.transcription_status === "Done" && recording.transcription) {
    transcriptionText.innerHTML = formatTranscription(recording.transcription);
  } else if (recording.transcription_status === "InProgress" || recording.transcription_status === "NotStarted") {
    transcriptionStatus.textContent = "Færdiggør transskription...";
    transcriptionStatus.classList.remove("hidden");
    if (previewText) {
      transcriptionText.innerHTML = formatTranscription(previewText);
    } else {
      transcriptionText.innerHTML =
        '<p class="empty-state">Vent venligst...</p>';
    }
  } else if (recording.transcription_status === "Failed") {
    transcriptionStatus.textContent = "Transskription fejlede";
    transcriptionStatus.classList.remove("hidden");
    transcriptionText.innerHTML =
      '<p class="empty-state">Prøv igen med knappen ovenfor</p>';
  } else {
    transcriptionText.innerHTML =
      '<p class="empty-state">Ingen transskription endnu</p>';
  }
}

// ─── Rename ───

function startRenaming() {
  if (!selectedRecordingId) return;
  const recording = recordings.find((r) => r.id === selectedRecordingId);
  if (!recording) return;

  detailTitle.classList.add("hidden");
  detailTitleInput.classList.remove("hidden");
  detailTitleInput.value = recording.title;
  detailTitleInput.focus();
  detailTitleInput.select();
}

async function finishRenaming() {
  const newTitle = detailTitleInput.value.trim();
  detailTitleInput.classList.add("hidden");
  detailTitle.classList.remove("hidden");

  if (!selectedRecordingId || !newTitle || !currentUid) return;

  const recording = recordings.find((r) => r.id === selectedRecordingId);
  if (!recording || newTitle === recording.title) return;

  try {
    await updateRecording(currentUid, selectedRecordingId, { title: newTitle });
    detailTitle.textContent = newTitle;
  } catch (e) {
    showError(`Kunne ikke omdøbe: ${e}`);
  }
}

function cancelRenaming() {
  detailTitleInput.classList.add("hidden");
  detailTitle.classList.remove("hidden");
}

// ─── Actions ───

async function copyTranscription() {
  const recording = recordings.find((r) => r.id === selectedRecordingId);
  if (recording?.transcription) {
    await navigator.clipboard.writeText(recording.transcription);
    copyBtn.textContent = "Kopieret!";
    setTimeout(() => {
      copyBtn.textContent = "Kopier";
    }, 2000);
  }
}

async function retranscribe() {
  if (!selectedRecordingId) return;
  const recording = recordings.find((r) => r.id === selectedRecordingId);
  if (!recording) return;
  try {
    await invoke("transcribe_recording", {
      id: selectedRecordingId,
      hasSystemAudio: recording.has_system_audio,
    });
  } catch (e) {
    console.error("Transskription fejlede:", e);
  }
}

async function deleteSelected() {
  if (!selectedRecordingId || !currentUid) return;
  if (!confirm("Er du sikker på, at du vil slette denne optagelse?")) return;

  audioPlayer.pause();
  audioPlayer.src = "";

  await deleteRecordingDoc(currentUid, selectedRecordingId);
  await invoke("delete_wav_files", { id: selectedRecordingId });

  selectedRecordingId = null;
  detailView.classList.add("hidden");
  detailEmpty.classList.remove("hidden");
}

// ─── Model Download ───

async function checkModel() {
  const settings: AppSettings = await invoke("get_settings");
  if (settings.transcription_provider === "openai") {
    modelOverlay.classList.add("hidden");
    return;
  }

  const ready: boolean = await invoke("check_model_status");
  if (!ready) {
    modelOverlay.classList.remove("hidden");
  }
}

async function startModelDownload() {
  downloadBtn.disabled = true;
  downloadProgress.classList.remove("hidden");
  try {
    await invoke("download_model");
    modelOverlay.classList.add("hidden");
  } catch (e) {
    console.error("Model download fejlede:", e);
    downloadBtn.disabled = false;
    downloadBtn.textContent = "Prøv igen";
  }
}

// ─── Event Listeners ───

function setupListeners() {
  recordBtn?.addEventListener("click", toggleRecording);

  // Settings
  settingsBtn?.addEventListener("click", async () => {
    const settings: AppSettings = await invoke("get_settings");
    openaiKeyInput.value = settings.openai_api_key ?? "";
    setTranscriptionProvider(settings.transcription_provider);
    settingsOverlay.classList.remove("hidden");
    openaiKeyInput.focus();
  });
  settingsSaveBtn.addEventListener("click", async () => {
    const transcriptionProvider = getTranscriptionProvider();
    if (transcriptionProvider === "openai" && !openaiKeyInput.value.trim()) {
      showWarning("OpenAI-transskription kræver en API-nøgle.");
      openaiKeyInput.focus();
      return;
    }

    await invoke("save_settings", {
      openaiApiKey: openaiKeyInput.value,
      transcriptionProvider,
    });
    settingsOverlay.classList.add("hidden");
    await checkModel();
  });
  settingsCancelBtn.addEventListener("click", () => {
    settingsOverlay.classList.add("hidden");
  });

  // Dictation feedback
  listen<boolean>("dictation-recording", (e) => {
    if (e.payload) {
      const dot = document.createElement("div");
      dot.id = "dictation-indicator";
      dot.className = "dictation-indicator";
      dot.title = "Dikterer... (slip Fn for at stoppe)";
      document.body.appendChild(dot);
    } else {
      document.getElementById("dictation-indicator")?.remove();
    }
  });

  logBtn?.addEventListener("click", toggleLogPanel);
  logCloseBtn?.addEventListener("click", () => logPanel.classList.add("hidden"));
  detailTitle?.addEventListener("click", startRenaming);
  detailTitleInput?.addEventListener("blur", finishRenaming);
  detailTitleInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") detailTitleInput.blur();
    if (e.key === "Escape") cancelRenaming();
  });
  copyBtn?.addEventListener("click", copyTranscription);
  retranscribeBtn?.addEventListener("click", retranscribe);
  deleteBtn?.addEventListener("click", deleteSelected);
  downloadBtn.addEventListener("click", startModelDownload);
  skipDownloadBtn.addEventListener("click", () =>
    modelOverlay.classList.add("hidden")
  );

  // Recording warnings
  listen<string>("recording-warning", (event) => {
    showWarning(event.payload);
  });

  // Tray events
  listen("tray-start-recording", () => {
    if (!isRecording) startRecording();
  });
  listen("tray-stop-recording", () => {
    if (isRecording) stopRecording();
  });

  // Live transcription chunks
  listen<{ id: string; text: string }>("transcription-chunk", (event) => {
    const { id, text } = event.payload;
    if (isRecording && currentRecordingId === id) {
      liveTranscriptionText += (liveTranscriptionText ? "\n" : "") + text;
      transcriptionText.innerHTML = formatTranscription(liveTranscriptionText);
      transcriptionText.scrollTop = transcriptionText.scrollHeight;
    }
  });

  // Transcription events → write to Firestore
  listen("transcription-started", (event) => {
    const id = event.payload as string;
    if (currentUid) {
      updateRecording(currentUid, id, {
        transcription_status: "InProgress",
      });
    }
  });

  listen<{ id: string; text: string }>("transcription-complete", (event) => {
    const { id, text } = event.payload;
    liveTranscriptionText = "";
    if (currentUid) {
      updateRecording(currentUid, id, {
        transcription: text,
        transcription_status: "Done",
      });
    }
  });

  listen("transcription-failed", (event) => {
    const id = event.payload as string;
    if (currentUid) {
      updateRecording(currentUid, id, {
        transcription_status: "Failed",
      });
    }
  });

  // Model download progress
  listen<DownloadProgress>("model-download-progress", (event) => {
    const { percent } = event.payload;
    progressFill.style.width = `${percent}%`;
    progressText.textContent = `${Math.round(percent)}%`;
  });
}

// ─── Permissions ───

const permissionsOverlay = document.getElementById("permissions-overlay")!;
const permissionsGrantedMsg = document.getElementById("permissions-granted-msg")!;
const permissionsRestartBtn = document.getElementById("permissions-restart-btn") as HTMLButtonElement;
const permissionsSettingsBtn = document.getElementById("permissions-settings-btn") as HTMLButtonElement;

let permissionPollInterval: number | null = null;

function startPermissionPolling() {
  permissionPollInterval = window.setInterval(async () => {
    const granted: boolean = await invoke("check_accessibility_permission");
    if (granted) {
      clearInterval(permissionPollInterval!);
      permissionsGrantedMsg.classList.remove("hidden");
      permissionsRestartBtn.classList.remove("hidden");
      permissionsSettingsBtn.classList.add("hidden");
    }
  }, 2000);
}

function setupPermissionsListeners() {
  listen("accessibility-permission-missing", () => {
    permissionsOverlay.classList.remove("hidden");
    invoke("open_accessibility_settings");
    startPermissionPolling();
  });

  permissionsSettingsBtn.addEventListener("click", () => {
    invoke("open_accessibility_settings");
  });

  permissionsRestartBtn.addEventListener("click", () => {
    invoke("restart_app");
  });
}

// ─── Auth & App Init ───

function startApp(uid: string) {
  currentUid = uid;
  authOverlay.classList.add("hidden");

  // Subscribe to recordings in Firestore (real-time)
  unsubRecordings = subscribeToRecordings(uid, (recs) => {
    recordings = recs;
    renderRecordingsList();
    if (selectedRecordingId) {
      const selected = recs.find((r) => r.id === selectedRecordingId);
      if (selected) updateTranscriptionView(selected);
    }
  });

  // Subscribe to recording control (remote start/stop)
  unsubControl = subscribeToControl(uid, async (control) => {
    if (control.action === "record" && !isRecording) {
      await startRecording();
      await resetControl(uid);
    } else if (control.action === "stop" && isRecording) {
      await stopRecording();
      await resetControl(uid);
    }
  });

  checkModel();
}

// ─── Init ───

window.addEventListener("beforeunload", () => cleanupSubscriptions());

window.addEventListener("DOMContentLoaded", async () => {
  setupListeners();
  setupPermissionsListeners();


  const user = await initAuth();
  if (user) {
    startApp(user.uid);
  } else {
    authOverlay.classList.remove("hidden");
  }

  loginBtn.addEventListener("click", async () => {
    loginBtn.disabled = true;
    loginBtn.textContent = "Logger ind...";
    authError.classList.add("hidden");
    try {
      const user = await signIn();
      startApp(user.uid);
    } catch (e: any) {
      authError.textContent = `Login fejlede: ${e.message || e}`;
      authError.classList.remove("hidden");
      loginBtn.disabled = false;
      loginBtn.textContent = "Log ind med Google";
    }
  });
});
