import { test } from 'node:test';
import assert from 'node:assert/strict';
import { connectionSettings, preferenceSnapshot } from './settings.ts';

test('provider switching preserves custom OpenAI settings and uses Gemini defaults', () => {
  const saved = { endpoint: 'wss://proxy.example/realtime', model: 'custom-deployment', 'auth-header': 'api-key' };
  assert.equal(connectionSettings('openai', saved).endpoint, saved.endpoint);
  assert.equal(connectionSettings('gemini', saved).model, 'gemini-3.8-live');
  const snapshot = preferenceSnapshot(saved, { provider: 'gemini', 'gemini-model': 'gemini-3.1-flash-live-preview' });
  assert.equal(connectionSettings('gemini', snapshot).model, 'gemini-3.1-flash-live-preview');
  assert.equal(connectionSettings('openai', snapshot).model, 'custom-deployment');
});

test('preferences exclude keys and context and migrate old proxy defaults', () => {
  const snapshot = preferenceSnapshot({ token: 'secret', context: 'private', provider: 'openai' }, { token: 'other-secret', opacity: '70' });
  assert.deepEqual(snapshot, { provider: 'openai', opacity: '70' });
  const old = connectionSettings('openai', { endpoint: 'ws://localhost:6655/openai/v1/realtime', 'auth-header': 'api-key', 'transcription-model': 'gpt-4o-mini-transcribe' });
  assert.equal(old.endpoint, 'ws://localhost:8080/v1/realtime');
  assert.equal(old['auth-header'], 'bearer');
  assert.equal(old['transcription-model'], 'whisper-1');
});
