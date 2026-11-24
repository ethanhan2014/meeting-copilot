# Meeting Copilot

A desktop assistant for Windows and macOS that suggests answers after the other meeting participant finishes speaking or asking a question. Your microphone is transcribed for conversation context only. Suggestions are displayed as text; the app never plays model audio.

## Download

Get the latest Windows executable or installer and the universal macOS DMG from [GitHub Releases](https://github.com/ethanhan2014/meeting-copilot/releases/latest).

- Windows 10/11 x64 requires Microsoft Edge WebView2 Runtime. The installer can download it.
- macOS 15 or newer supports Intel and Apple Silicon. Open the DMG, copy Meeting Copilot to Applications, and allow Screen & System Audio Recording and Microphone access in System Settings → Privacy & Security. Restart the app if macOS requests it. The macOS package is ad hoc signed and has not been notarized with an Apple Developer account.

## Configure

Open Settings and select your AI provider:

| Setting | Google Gemini Live | OpenAI Realtime / SAP proxy |
| --- | --- | --- |
| Endpoint | Filled automatically with Google's Live WebSocket endpoint | Your proxy's Realtime WebSocket endpoint |
| Key | Google AI Studio API key, entered locally | Locally configured proxy token, entered locally |
| Meeting model | gemini-3.8-live by default | gpt-realtime by default |
| Microphone transcription | gemini-3.5-transcribe-live, with text output only | whisper-1 by default; automatic suggestions disabled |

The Gemini model field is editable and also suggests gemini-3.1-flash-live-preview and gemini-2.5-flash-native-audio-preview-12-2025. Availability depends on your key and Google's model lifecycle. Get a [Google AI Studio API key](https://aistudio.google.com/api-keys). No proxy is needed for Gemini.

An OpenAI-compatible proxy must support WebSocket upgrades. The local default is ws://localhost:8080/v1/realtime with Bearer authentication. Remote endpoints require wss://. Older default ports 6655 and 8081 migrate to 8080; custom endpoints are preserved. The optional SAP bridge in scripts/ expects an existing sap-ai-proxy checkout.

Choose the playback device selected in Teams and your microphone. On macOS, meeting playback uses System audio (all applications); microphone selection remains available. Use a headset to keep meeting playback out of your microphone.

Enter your role, participant roles, goals, and facts as session context. Start with a 0.9-second wait after speech; increase it if natural pauses cause premature suggestions. Pauses longer than this interval can end a speaking turn.

## Recording and sessions

1. Start recording to capture meeting playback and your microphone separately. Both sources have labelled transcripts; only completed meeting turns receive suggested answers.
2. Stop recording to pause. Capture stops immediately and final transcripts and suggestions drain for two to fifteen seconds. Unfinished turns are marked. Restart continues the same session with recent spoken history.
3. New session starts a separate conversation with empty context. Earlier sessions remain selectable when recording is stopped.
4. Export transcripts saves the selected session as Markdown through a native Save As dialog. Exports contain every captured turn, timestamps, source labels, and associated suggested answers. Context and keys are excluded. Export during recording creates a snapshot.

The latest 80 turns are displayed; exports retain all turns. Sessions, context, and transcripts stay in memory and are lost on closing or crashing. Normal closing warns about unexported transcripts. Export each session you want to keep.

While recording, the window stays on top with adjustable opacity. Copy copies the latest answer. Drag the title bar to move the window. Stopping restores full opacity.

## Audio, usage, and privacy

- Windows uses WASAPI; macOS uses ScreenCaptureKit. Audio reaches the provider as mono PCM16 at 24 kHz in 20 ms chunks. Playback capture includes applications using the selected Windows output, or all system playback on macOS. It does not identify individual meeting speakers.
- Recording opens two provider connections. Completed microphone transcripts are sent to the meeting model as labelled context. AI suggestions are never treated as spoken words.
- Gemini's conversational Live models require audio generation internally. Output transcription supplies the displayed answer and generated audio is discarded. Generated audio still incurs Google usage charges. The microphone uses a transcription-only model. For direct audio input with text-only generation, use gpt-realtime.
- Keys are entered locally, stay in memory, and are cleared when switching providers. Keys, context, and transcripts are never saved in preferences. Provider connection preferences are saved separately.
- Audio and session context go directly to Google for Gemini, or through your configured OpenAI proxy. Record with participants' permission. Connection or capture errors stop both streams; restart recording to reconnect. Provider duration and usage limits apply. Review suggestions before speaking.

## Build

Install Node.js 22 and Rust. Windows requires Visual Studio C++ Build Tools; macOS requires Xcode with the macOS 15 SDK or newer.

```sh
npm ci
```

Windows:

```sh
npm run build:windows
```

Universal macOS app:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run build:macos
```

Packages are produced under src-tauri/target/TARGET/release/bundle/. The release workflow builds and tests both platforms before publishing the installer, standalone executable, and universal DMG. A version tag must match package.json, Cargo.toml, and tauri.conf.json. Manual workflow dispatch reads the package version and publishes that tag at the tested commit.

## Verification

```sh
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml -- --skip windows_capture_delivers_pcm_and_stops
```

The Windows capture test requires actual audio devices. Optional bridge tests use the existing proxy checkout's ws dependency.

An authenticated synthetic Gemini check passed with concurrent meeting and microphone connections: microphone speech produced a transcript without suggestions; meeting speech produced the correct answer from supplied test facts about 1.45 seconds after speech ended. Sanitized observed frames are replayed by a Rust regression test. Browser tests with mocked native IPC verified both transcript sources, meeting-only answers, provider switching, key handling, stop, and reload.

macOS audio conversion, downmixing, resampling, and chunk tests pass. Native builds and unit tests run in CI. A physical Mac's capture permissions, native recording, and a real Teams meeting have not yet been validated. Earlier Windows capture checks used a local mock server that discarded audio.

References: [OpenAI Realtime conversations](https://developers.openai.com/api/docs/guides/realtime-conversations), [SAP AI SDK Realtime](https://sap.github.io/ai-sdk/docs/js/openai/realtime), [Gemini Live WebSocket reference](https://ai.google.dev/api/live), [Gemini Live transcription](https://aistudio.google.com/docs/live-api/live-transcribe), [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit).
