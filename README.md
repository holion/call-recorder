# Call Recorder

Opkaldsoptager til salgsteam. Optager mikrofon og systemlyd, transskriberer automatisk med en lokal Whisper-model, og gemmer alt i Firestore.

Kører som en macOS menubar-app bygget med Tauri 2 + TypeScript.

## Features

- Optag mikrofon + systemlyd (via ScreenCaptureKit på macOS)
- Live transskription under optagelse med speaker labels (Sælger / Lead)
- Lokal Whisper-model — ingen lyd sendes til tredjepart
- Google login via system browser
- Optagelser synkroniseret til Firestore i real-time
- Fjernbetjening: start/stop optagelse fra Firestore (f.eks. fra en webapp)
- Menubar tray-ikon med hurtig adgang

## Forudsætninger

- [Node.js](https://nodejs.org/) (>= 18)
- [Rust](https://www.rust-lang.org/tools/install) (stable)
- [Tauri CLI](https://v2.tauri.app/start/prerequisites/) (`npm install -g @tauri-apps/cli`)
- [Firebase CLI](https://firebase.google.com/docs/cli) (valgfrit, til emulators)
- macOS 13+ (systemlyd kræver ScreenCaptureKit)

## Setup

```bash
# Installer dependencies
npm install

# Start i development mode
npm run tauri dev
```

### Firebase Emulators (valgfrit)

```bash
npm run emulators
```

Starter Auth- og Firestore-emulatorer på `127.0.0.1:9099` og `127.0.0.1:8083`.

## Projektstruktur

```
src/                        # Frontend (TypeScript)
  main.ts                   # App-logik, UI, event listeners
  auth.ts                   # Google OAuth via Tauri backend
  db.ts                     # Firestore CRUD og real-time subscriptions
  firebase.ts               # Firebase config og init

src-tauri/src/              # Backend (Rust)
  lib.rs                    # Tauri commands og app setup
  google_auth.rs            # OAuth flow via system browser
  audio/                    # Optagelse (mic + system)
  transcription/            # Whisper-integration og model download
  state.rs                  # App state
  logging.rs                # Log panel
```

## Nyttige kommandoer

### Nulstil login

Firebase gemmer auth-token i webviewets lokale storage. Slet dem og genstart appen for at blive bedt om at logge ind igen:

```bash
rm -rf ~/Library/WebKit/dk.holion.call-recorder
rm -rf ~/Library/Application\ Support/dk.holion.call-recorder/WebKit
```

## Build

```bash
npm run tauri build
```

Producerer en `.dmg` / `.app` i `src-tauri/target/release/bundle/`.

## Release

Brug release-scriptet til at bumpe versionen, synkronisere Tauri-versionen, lave commit og oprette git-tag:

```bash
npm run release -- patch
npm run release -- minor
npm run release -- major
```

Scriptet laver et commit som `Release vX.Y.Z` og et tag `vX.Y.Z`. Push derefter commit og tag for at starte GitHub Actions-releasen.

## Humio Logging

Appen kan valgfrit sende backend-logs til Humio / Falcon LogScale via structured ingest-endpointet.

Sæt disse compile-time variabler:

```bash
HUMIO_INGEST_URL=https://<your-host>/api/v1/ingest/humio-structured
HUMIO_INGEST_TOKEN=<ingest-token>
HUMIO_ENV=dev
```

Logs sendes altid med tags `system=anna` og `environment=<dev|prod>`. Lokale builds falder tilbage til `dev`, og GitHub Actions release-builds sætter `prod`.

Hvis de er sat under build, bliver logs batch'et og sendt i baggrunden. Hvis de mangler, bruges kun den lokale log-buffer i appen.
