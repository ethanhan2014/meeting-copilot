import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { createSession, updateTurns, finishTurns, exportSession, sourceLabel, type MeetingEvent, type Turn, type AudioSource } from './state';
import { connectionSettings, connectionFields, commonFields, preferenceSnapshot, type Provider } from './settings';
import './style.css';

const icons: Record<string, string> = {
  spark: '<path d="m12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5L12 3Z"/>',
  settings: '<path d="M4 7h16M4 17h16"/><circle cx="9" cy="7" r="3"/><circle cx="15" cy="17" r="3"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>', minus: '<path d="M5 12h14"/>',
  sound: '<path d="m11 5-6 4H2v6h3l6 4V5Zm4 3a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14"/>',
  arrow: '<path d="M5 12h14m-5-5 5 5-5 5"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V4H4v12h4"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
};
const icon = (name: string) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name]}</svg>`;
const meter = (id: string, label: string) => `<div id="${id}" class="meter" aria-label="${label}"><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i><i></i></div>`;
document.querySelector<HTMLDivElement>('#app')!.innerHTML = `
<main class="shell">
  <header class="titlebar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region><span class="brand-icon">${icon('spark')}</span><span data-tauri-drag-region>meeting<span class="brand-light">copilot</span></span><span class="version">REALTIME</span></div>
    <div class="window-actions"><button id="settings-toggle" class="icon-button" aria-label="Open settings" title="Settings">${icon('settings')}</button><button id="minimize" class="icon-button" aria-label="Minimize">${icon('minus')}</button><button id="close" class="icon-button close" aria-label="Close">${icon('close')}</button></div>
  </header>
  <div class="statusbar"><span class="status"><i id="status-dot"></i><span id="status-text" role="status">Ready when you are</span></span><span id="clock">00:00</span></div>
  <div class="sessionbar"><label for="session-select">Session</label><select id="session-select" aria-label="Select session"></select><button id="new-session" class="text-button">+ New session</button><button id="export" class="text-button" disabled>Export transcripts</button></div>
  <div id="export-status" class="export-status" role="status" hidden></div>
  <div id="error" class="error" role="alert" hidden></div>
  <section id="settings" class="settings" hidden>
    <div class="section-heading"><span>Connection & session context</span><span id="provider-label" class="small-label">YOUR AI PROXY</span></div>
    <label>AI provider<select id="provider"><option value="openai">OpenAI Realtime / SAP proxy</option><option value="gemini">Google Gemini Live</option></select></label>
    <label>Realtime WebSocket URL<input id="endpoint" type="url" value="ws://localhost:8080/v1/realtime" spellcheck="false" autocomplete="off" /></label>
    <div class="form-row"><label id="auth-field">Authentication<select id="auth-header"><option value="bearer" selected>Authorization: Bearer</option><option value="api-key">api-key</option><option value="x-api-key">x-api-key</option></select></label><label><span id="key-label">Proxy token</span><input id="token" type="password" autocomplete="off" placeholder="Key only; the app adds Bearer" /></label></div>
    <div class="form-row"><label>Realtime model<input id="model" value="gpt-realtime" spellcheck="false" /></label><label id="transcription-field">Transcription model<input id="transcription-model" value="whisper-1" spellcheck="false" /></label></div>
    <datalist id="gemini-models"><option value="gemini-3.8-live"></option><option value="gemini-3.1-flash-live-preview"></option><option value="gemini-2.5-flash-native-audio-preview-12-2025"></option></datalist>
    <p id="gemini-note" class="settings-note" hidden>Suggestions appear as text; generated audio is discarded and never played. Google still charges for generated audio. Your Gemini API key stays in memory.</p>
    <label>Teams speaker / headset<select id="device"><option value="">Default playback device</option></select></label>
    <label>Your microphone (context only)<select id="microphone-device"><option value="">Default microphone</option></select></label>
    <button id="refresh-devices" class="text-button" type="button">Refresh audio devices</button>
    <label>Context for <span id="context-session">this session</span><textarea id="context" rows="4" maxlength="10000" aria-describedby="context-count" placeholder="Your role, other participants' roles, project facts, goals, or useful background…"></textarea><span id="context-count" class="context-count">0 / 10,000 characters</span></label>
    <label>Wait after speech<select id="silence"><option value="600">0.6 seconds · quick</option><option value="900" selected>0.9 seconds · balanced</option><option value="1500">1.5 seconds · fewer interruptions</option><option value="2200">2.2 seconds · long pauses</option></select></label>
    <p class="settings-note">Suggestions appear after the other person finishes speaking. Your microphone supplies conversation context only. Context stays in this session and is not included in exports. Use a headset to keep meeting playback out of your microphone.</p>
    <button id="save-settings" class="primary-button">Done ${icon('arrow')}</button>
  </section>
  <section id="meeting" class="meeting">
    <div id="welcome" class="welcome"><div class="orb"><span>${icon('spark')}</span><div class="orb-ring"></div></div><span class="eyebrow">A LITTLE HELP, RIGHT ON TIME</span><h1>Stay in the conversation.</h1><p>Meeting and microphone transcripts.<br/>Suggested answers after the other person finishes.</p><div class="steps"><span><b>01</b> Add session context</span><span><b>02</b> Select both devices</span><span><b>03</b> Start listening</span></div><button id="setup" class="text-button">Set up this session ${icon('arrow')}</button></div>
    <div id="conversation" class="conversation" hidden>
      <section class="transcript-pane"><div class="pane-heading"><span><i class="tiny-dot"></i> LIVE TRANSCRIPTS</span><span id="speech-label">Waiting for speech</span></div><div id="transcripts" class="turns" role="log" aria-label="Meeting and microphone transcripts"></div></section>
      <section class="reply-pane"><div class="pane-heading"><span>${icon('spark')} SUGGESTED ANSWER</span><button id="copy" class="icon-button" title="Copy latest answer" aria-label="Copy latest answer">${icon('copy')}</button></div><div id="replies" class="turns replies" role="log" aria-live="polite" aria-label="Suggested answers"></div></section>
    </div>
  </section>
  <footer>
    <div class="audio-row"><span>${icon('sound')} <span id="device-label">Meeting audio</span></span>${meter('meter-meeting','Meeting audio level')}</div>
    <div class="audio-row"><span>${icon('sound')} <span id="microphone-label">Your microphone</span></span>${meter('meter-microphone','Microphone audio level')}</div>
    <div class="control-row"><button id="record" class="record-button" disabled><span class="record-symbol"></span><span id="record-label">Start recording</span></button><label class="opacity-control" for="opacity"><span>Opacity <b id="opacity-label">65%</b></span><input id="opacity" type="range" min="30" max="100" value="65" /></label></div>
    <div class="footnote"><span>Session history stays in memory · export before closing</span><span id="audio-destination">Both audio inputs go to your proxy while recording</span></div>
  </footer>
</main>`;

const el = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => el<HTMLInputElement>(id);
const desktop = isTauri();
let active = false, busy = false, exporting = false, closing = false;
let runId = '', startedAt = 0;
let runEnded: Promise<void> = Promise.resolve();
let endRun: (() => void) | undefined;
const sessions = [createSession('Session 1')];
let current = sessions[0];
const speaking = new Set<AudioSource>();
let saved: Record<string, string> = {};
try { const data = JSON.parse(localStorage.getItem('meeting-copilot-settings') || '{}'); if (data && typeof data === 'object') saved = data; } catch { /* Ignore corrupt local preferences. */ }
let provider: Provider = saved.provider === 'gemini' ? 'gemini' : 'openai';
for (const id of ['silence', 'opacity']) if (typeof saved[id] === 'string') input(id).value = saved[id];
applyProvider();

function applyProvider() {
  input('provider').value = provider;
  for (const [id, value] of Object.entries(connectionSettings(provider, saved))) input(id).value = value;
  const gemini = provider === 'gemini';
  el('auth-field').hidden = gemini; el('transcription-field').hidden = gemini;
  el('gemini-note').hidden = !gemini;
  input('endpoint').readOnly = gemini;
  el('key-label').textContent = gemini ? 'Gemini API key' : 'Proxy token';
  input('token').placeholder = gemini ? 'Enter your Google AI Studio API key' : 'Key only; the app adds Bearer';
  el('provider-label').textContent = gemini ? 'GOOGLE GEMINI' : 'YOUR AI PROXY';
  el('audio-destination').textContent = gemini ? 'Both audio inputs go to Google while recording' : 'Both audio inputs go to your proxy while recording';
  if (gemini) input('model').setAttribute('list', 'gemini-models');
  else input('model').removeAttribute('list');
}

function showError(message: string) { el('error').textContent = message; el('error').hidden = false; }
function showSettings(open: boolean) {
  el('settings').hidden = !open; el('meeting').hidden = open;
  el('settings-toggle').setAttribute('aria-expanded', String(open));
}
function savePreferences() {
  saved = preferenceSnapshot(saved, Object.fromEntries([
    ...commonFields.map(id => [id, input(id).value]),
    ...connectionFields.map(id => [provider + '-' + id, input(id).value]),
  ]));
  try { localStorage.setItem('meeting-copilot-settings', JSON.stringify(saved)); }
  catch { showError('Preferences could not be saved. You can still record.'); }
}
function controls() {
  input('record').disabled = busy;
  input('new-session').disabled = busy || exporting;
  input('session-select').disabled = active || busy || exporting;
  input('export').disabled = exporting || busy || !current.turns.length;
  for (const id of ['provider','endpoint','auth-header','token','model','transcription-model','device','microphone-device','context','silence','refresh-devices']) input(id).disabled = active || busy;
}
function setStatus(status: string) {
  const labels: Record<string, string> = { idle: 'Ready when you are', connecting: 'Connecting both audio streams…', listening: 'Listening to meeting and microphone', finishing: 'Finishing transcripts and replies…', error: 'Recording stopped' };
  el('status-text').textContent = labels[status] || status;
  el('status-dot').className = status;
  el('record-label').textContent = status === 'finishing' ? 'Finishing…' : active ? 'Stop recording' : 'Start recording';
  el('record').classList.toggle('recording', active);
  if (!active) { speaking.clear(); el('speech-label').textContent = 'Paused'; document.querySelectorAll('.meter i').forEach(bar => bar.classList.remove('lit')); }
  controls();
}
async function overlay() {
  const opacity = Number(input('opacity').value) / 100;
  el('opacity-label').textContent = `${Math.round(opacity * 100)}%`;
  if (desktop) await invoke('set_overlay', { opacity: active ? opacity : 1, pinned: active });
}
function updateContext() {
  current.context = el<HTMLTextAreaElement>('context').value;
  el('context-count').textContent = `${current.context.length.toLocaleString()} / 10,000 characters`;
}
function renderTurns(container: HTMLElement, turns: Turn[], reply: boolean) {
  const atBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 80;
  const top = container.scrollTop;
  container.replaceChildren();
  // shortcut: only the latest 80 turns are displayed; exports retain the entire session.
  for (const turn of turns.filter(t => !reply || t.reply).slice(-80)) {
    const data = reply ? turn.reply! : turn;
    const row = document.createElement('article');
    row.className = `turn ${data.done || (!reply && turn.incomplete) ? 'complete' : 'streaming'} ${turn.source === 'microphone' ? 'microphone-turn' : ''}`;
    const meta = document.createElement('span'); meta.className = 'turn-meta';
    const time = new Date(turn.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    meta.textContent = `${reply ? 'After ' : ''}${sourceLabel(turn.source)} · ${time}${reply && turn.reply?.interrupted ? ' · interrupted' : !data.done ? reply ? ' · composing' : turn.incomplete ? ' · incomplete' : ' · transcribing' : ''}`;
    const text = document.createElement('p'); text.textContent = data.text || (reply ? 'Thinking…' : 'Listening…');
    row.append(meta, text); container.append(row);
  }
  if (!container.children.length) { const hint = document.createElement('p'); hint.className = 'empty-hint'; hint.textContent = reply ? 'An answer appears after the other person finishes speaking.' : 'Waiting for speech from either audio input.'; container.append(hint); }
  container.scrollTop = atBottom ? container.scrollHeight : top;
}
function renderSession() {
  const select = el<HTMLSelectElement>('session-select'); select.replaceChildren();
  for (const session of sessions) select.add(new Option(session.title, session.id));
  select.value = current.id;
  el<HTMLTextAreaElement>('context').value = current.context;
  el('context-session').textContent = current.title;
  updateContext();
  el('welcome').hidden = current.turns.length > 0;
  el('conversation').hidden = current.turns.length === 0;
  renderTurns(el('transcripts'), current.turns, false); renderTurns(el('replies'), current.turns, true);
  controls();
}
async function refreshDevices() {
  if (!desktop) return;
  try {
    const devices = await invoke<{ outputs: { id: string; name: string }[]; microphones: { id: string; name: string }[] }>('audio_devices');
    for (const [id, options, label] of [['device',devices.outputs,'Default playback device'],['microphone-device',devices.microphones,'Default microphone']] as const) {
      const select = el<HTMLSelectElement>(id); const previous = select.value || saved[id];
      select.replaceChildren(new Option(label, '')); for (const device of options) select.add(new Option(device.name, device.id));
      if (options.some(d => d.id === previous)) select.value = previous;
    }
  } catch (error) { showError(String(error)); }
}
function receive(event: MeetingEvent) {
  if (event.sessionId !== runId) return;
  if (event.kind === 'error' || (event.kind === 'status' && event.status === 'idle')) {
    active = false; const finished = finishTurns(current.turns);
    if (finished !== current.turns) { current.turns = finished; current.exported = false; }
    setStatus(event.kind === 'error' ? 'error' : 'idle');
    if (event.message) showError(event.message);
    renderSession(); endRun?.(); void overlay().catch(error => showError(String(error)));
  } else if (event.kind === 'status') setStatus(event.status || 'listening');
  else if (event.kind === 'level') {
    const source = event.source ?? 'meeting'; const level = Math.min(1, (event.level || 0) * 5);
    el(`meter-${source}`).querySelectorAll('i').forEach((bar, index) => bar.classList.toggle('lit', index / 10 < level));
  } else {
    if (event.kind === 'speech') {
      const source = event.source ?? 'meeting'; if (event.active) speaking.add(source); else speaking.delete(source);
      el('speech-label').textContent = speaking.size ? [...speaking].map(sourceLabel).join(' + ') + ' speaking' : 'Waiting for speech';
      if (!event.active) return;
    }
    const next = updateTurns(current.turns, event.kind === 'speech' ? { ...event, kind: 'turn' } : event);
    if (next !== current.turns) { current.turns = next; current.exported = false; el('export-status').hidden = true; }
    el('welcome').hidden = true; el('conversation').hidden = false;
    renderTurns(el('transcripts'), current.turns, false); renderTurns(el('replies'), current.turns, true); controls();
  }
}
async function stop() {
  if (!desktop) return;
  if (active) setStatus('finishing');
  await invoke('stop_recording'); await runEnded; active = false; setStatus('idle'); await overlay();
}
el('record').addEventListener('click', async () => {
  if (busy) return;
  if (!desktop) { showError('Open the desktop app to record audio.'); return; }
  busy = true; controls(); el('error').hidden = true; el('export-status').hidden = true;
  try {
    if (active) await stop();
    else {
      if (!input('endpoint').value.trim() || !input('token').value.trim()) { showSettings(true); throw new Error(provider === 'gemini' ? 'Enter your Gemini API key in Settings.' : 'Enter the proxy URL and key in Settings. Enter the key alone; the app adds Bearer.'); }
      updateContext(); savePreferences();
      runId = crypto.randomUUID(); startedAt = Date.now(); speaking.clear();
      runEnded = new Promise(resolve => { endRun = resolve; });
      // shortcut: resumed model sessions receive recent spoken history; add summaries if longer context is needed.
      const history = [...current.turns.filter(t => t.done && t.text).map(t => `${sourceLabel(t.source)}: ${t.text}`).join('\n')].slice(-10000).join('');
      const settings = {
        provider,
        endpoint: input('endpoint').value, token: input('token').value, authHeader: input('auth-header').value,
        model: input('model').value, transcriptionModel: input('transcription-model').value,
        deviceId: input('device').value, microphoneDeviceId: input('microphone-device').value,
        context: current.context, history, silenceMs: Number(input('silence').value),
      };
      active = true; setStatus('connecting'); await overlay();
      await invoke('start_recording', { sessionId: runId, settings });
      el('welcome').hidden = true; el('conversation').hidden = false;
      el('clock').textContent = '00:00';
      el('device-label').textContent = el<HTMLSelectElement>('device').selectedOptions[0]?.textContent || 'Meeting audio';
      el('microphone-label').textContent = el<HTMLSelectElement>('microphone-device').selectedOptions[0]?.textContent || 'Your microphone';
      showSettings(false);
    }
  } catch (error) { endRun?.(); active = false; setStatus('error'); showError(String(error)); await overlay().catch(() => {}); }
  finally { busy = false; controls(); }
});
el('new-session').addEventListener('click', async () => {
  if (busy || exporting) return;
  busy = true; controls();
  try {
    if (active) await stop();
    current = createSession(`Session ${sessions.length + 1}`); sessions.push(current); runId = '';
    el('clock').textContent = '00:00'; el('export-status').hidden = true; el('error').hidden = true;
    renderSession(); showSettings(true);
  } catch (error) { showError(String(error)); }
  finally { busy = false; controls(); }
});
el('session-select').addEventListener('change', () => {
  if (active || busy) return;
  current = sessions.find(session => session.id === el<HTMLSelectElement>('session-select').value)!;
  runId = ''; el('export-status').hidden = true; renderSession();
});
el('export').addEventListener('click', async () => {
  if (!desktop || exporting || !current.turns.length) return;
  exporting = true; controls();
  const session = current; const snapshot = session.turns;
  try {
    const filename = `Meeting-Copilot-${session.title.replaceAll(' ', '-')}-${session.createdAt.slice(0,10)}.md`;
    const path = await invoke<string | null>('export_transcript', { text: exportSession(session), filename });
    if (path) { if (session.turns === snapshot) session.exported = true; el('export-status').textContent = `${session.title} snapshot exported.`; el('export-status').hidden = false; }
  } catch (error) { showError(String(error)); }
  finally { exporting = false; controls(); }
});
el('settings-toggle').addEventListener('click', () => showSettings(el('settings').hidden));
el('provider').addEventListener('change', () => {
  const next = input('provider').value as Provider;
  savePreferences();
  provider = next;
  input('token').value = '';
  applyProvider();
  savePreferences();
});
el('setup').addEventListener('click', () => showSettings(true));
el('save-settings').addEventListener('click', () => { updateContext(); savePreferences(); showSettings(false); });
el('context').addEventListener('input', updateContext);
el('refresh-devices').addEventListener('click', () => void refreshDevices());
el('minimize').addEventListener('click', () => { if (desktop) void getCurrentWindow().minimize().catch(error => showError(String(error))); });
el('close').addEventListener('click', () => { if (desktop) void getCurrentWindow().close().catch(error => showError(String(error))); });
el('copy').addEventListener('click', async () => {
  const text = current.turns.findLast(turn => turn.reply?.text)?.reply?.text;
  if (!text) return;
  try { await navigator.clipboard.writeText(text); el('copy').innerHTML = icon('check'); setTimeout(() => { el('copy').innerHTML = icon('copy'); }, 1500); }
  catch { showError('Clipboard access failed. Select and copy the reply text.'); }
});
input('opacity').addEventListener('input', () => void overlay().catch(error => showError(String(error))));
input('opacity').addEventListener('change', savePreferences);
setInterval(() => { if (active) { const seconds = Math.floor((Date.now() - startedAt) / 1000); el('clock').textContent = `${String(Math.floor(seconds / 60)).padStart(2,'0')}:${String(seconds % 60).padStart(2,'0')}`; } }, 1000);

async function init() {
  busy = true; renderSession(); controls();
  try {
    if (desktop) {
      await listen<MeetingEvent>('meeting-event', event => receive(event.payload));
      await getCurrentWindow().onCloseRequested(async event => {
        if (closing || busy || exporting) { event.preventDefault(); return; }
        closing = true; busy = true; controls();
        try {
          if (active) await stop();
          if (sessions.some(session => session.turns.length && !session.exported) && !confirm('Unexported session transcripts will be lost when the app closes. Close anyway?')) event.preventDefault();
          else input('token').value = '';
        } catch (error) { event.preventDefault(); showError(String(error)); }
        finally { closing = false; busy = false; controls(); }
      });
    }
    await refreshDevices(); await overlay();
  } catch (error) { showError(String(error)); }
  finally { busy = false; controls(); }
}
void init();
