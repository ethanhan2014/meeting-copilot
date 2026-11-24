export type AudioSource = 'meeting' | 'microphone';
export type MeetingEvent = {
  sessionId: string;
  kind: 'status' | 'error' | 'speech' | 'turn' | 'transcript' | 'reply' | 'reply_end' | 'level';
  source?: AudioSource;
  id?: string;
  turnId?: string;
  text?: string;
  done?: boolean;
  replace?: boolean;
  status?: string;
  message?: string;
  active?: boolean;
  level?: number;
};
export type Reply = { id: string; text: string; done: boolean; interrupted: boolean };
export type Turn = {
  id: string; source: AudioSource; at: string; text: string; done: boolean;
  incomplete: boolean; reply?: Reply;
};
export type MeetingSession = {
  id: string; title: string; createdAt: string; context: string; turns: Turn[]; exported: boolean;
};
export const sourceLabel = (source: AudioSource) => source === 'microphone' ? 'Your microphone' : 'Meeting audio';

export function createSession(title: string): MeetingSession {
  return { id: crypto.randomUUID(), title, createdAt: new Date().toISOString(), context: '', turns: [], exported: false };
}

export function updateTurns(turns: Turn[], event: MeetingEvent): Turn[] {
  if (!event.id) return turns;
  const id = `${event.sessionId}:${event.id}`;
  const turnId = event.turnId ? `${event.sessionId}:${event.turnId}` : undefined;
  const source = event.source ?? 'meeting';
  const isReply = event.kind === 'reply' || event.kind === 'reply_end';
  if (isReply && source === 'microphone') return turns;
  const existing = isReply
    ? turns.findIndex(t => t.source === source && (turnId ? t.id === turnId : t.reply?.id === id))
    : turns.findIndex(t => t.source === source && t.id === id);
  if (isReply && existing < 0 && !event.turnId) return turns;
  const turn: Turn = existing >= 0
    ? { ...turns[existing], reply: turns[existing].reply ? { ...turns[existing].reply! } : undefined }
    : { id: turnId ?? id, source, at: new Date().toISOString(), text: '', done: false, incomplete: false };
  if (isReply) {
    const reply = turn.reply?.id === id ? turn.reply : { id, text: '', done: false, interrupted: false };
    turn.reply = reply;
    if (event.kind === 'reply_end') {
      reply.done = true;
      reply.interrupted = event.status === 'cancelled' || event.status === 'incomplete';
    } else if (event.done) {
      reply.text = event.text ?? '';
      reply.done = true;
    } else if (!reply.done) reply.text += event.text ?? '';
  } else if (event.kind === 'transcript') {
    if (event.done) { turn.text = event.text ?? ''; turn.done = true; turn.incomplete = false; }
    else if (!turn.done) turn.text = event.replace ? event.text ?? '' : turn.text + (event.text ?? '');
  }
  const next = turns.slice();
  if (existing >= 0) next[existing] = turn;
  else next.push(turn);
  return next;
}

export function finishTurns(turns: Turn[]): Turn[] {
  if (!turns.some(t => !t.done || (t.reply && !t.reply.done))) return turns;
  return turns.map(t => ({ ...t, incomplete: !t.done,
    reply: t.reply && !t.reply.done ? { ...t.reply, done: true, interrupted: true } : t.reply,
  }));
}

export function exportSession(session: MeetingSession): string {
  const quote = (text: string) => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
    .split('\n').map(line => `> ${line}`).join('\n');
  const sections = session.turns.map(turn => {
    const time = new Date(turn.at).toISOString();
    let text = `### ${sourceLabel(turn.source)} — ${time}\n\n${quote(turn.text || '(Transcript not finalized)')}\n`;
    if (!turn.done || turn.incomplete) text += '\n*Transcript incomplete.*\n';
    if (turn.reply) {
      text += `\n**AI suggested answer**\n\n${quote(turn.reply.text || '(Suggestion not completed)')}\n`;
      if (!turn.reply.done || turn.reply.interrupted) text += '\n*Suggestion incomplete or interrupted.*\n';
    }
    return text;
  });
  return `# ${session.title}\n\nStarted: ${session.createdAt}\n\nAI suggestions are not spoken transcript.\n\n${sections.join('\n')}`;
}
