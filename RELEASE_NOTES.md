Meeting Copilot suggests text answers after the other participant finishes speaking. Your microphone provides conversation context only.

- Windows 10/11 x64: download the standalone executable or the installer. WebView2 is required; the installer can download it.
- macOS 15 or newer, Intel and Apple Silicon: open the universal DMG and copy Meeting Copilot to Applications. Allow Screen & System Audio Recording and Microphone access in System Settings, then restart the app if macOS requests it.
- Select Google Gemini Live or your OpenAI Realtime proxy in Settings. Enter your API key locally; keys are never saved in preferences.
- Gemini uses a conversational Live model for meeting audio and a transcription-only model for your microphone. Responses are displayed as text; generated meeting-model audio is discarded and still counts toward provider usage.

The macOS app is ad hoc signed and is not notarized with an Apple Developer account. Builds and protocol tests run in CI; native macOS recording and a live Teams meeting have not been validated on a physical Mac.
