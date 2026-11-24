export type Provider = 'openai' | 'gemini';
export const connectionFields = ['endpoint', 'auth-header', 'model', 'transcription-model'] as const;
export const commonFields = ['provider', 'device', 'microphone-device', 'silence', 'opacity'] as const;
const preferenceFields = [...commonFields, ...connectionFields,
  ...['openai', 'gemini'].flatMap(provider => connectionFields.map(field => provider + '-' + field))];

export function connectionSettings(provider: Provider, saved: Record<string, string>): Record<string, string> {
  const defaults = provider === 'gemini'
    ? { endpoint: 'wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent',
        'auth-header': 'bearer', model: 'gemini-3.8-live', 'transcription-model': '' }
    : { endpoint: 'ws://localhost:8080/v1/realtime', 'auth-header': 'bearer', model: 'gpt-realtime', 'transcription-model': 'whisper-1' };
  const settings = Object.fromEntries(connectionFields.map(field => [field,
    saved[provider + '-' + field] ?? (provider === 'openai' ? saved[field] : undefined) ?? defaults[field]]));
  if (provider === 'openai') {
    if (['ws://localhost:8081/v1/realtime', 'ws://localhost:6655/openai/v1/realtime'].includes(settings.endpoint)) {
      settings.endpoint = defaults.endpoint; settings['auth-header'] = 'bearer';
    }
    if (settings.endpoint === defaults.endpoint && settings['transcription-model'] === 'gpt-4o-mini-transcribe') settings['transcription-model'] = 'whisper-1';
  } else {
    settings.endpoint = defaults.endpoint;
  }
  return settings;
}

export function preferenceSnapshot(saved: Record<string, string>, changes: Record<string, string>): Record<string, string> {
  const values = { ...saved, ...changes };
  return Object.fromEntries(preferenceFields.filter(field => typeof values[field] === 'string').map(field => [field, values[field]]));
}
