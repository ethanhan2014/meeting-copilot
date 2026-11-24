import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createSession, updateTurns, exportSession, finishTurns } from './state.ts';

test('new sessions preserve earlier data and start with empty context', () => {
  const first = createSession('Session 1'); first.context = 'Private role information';
  first.turns = updateTurns([], { sessionId: 'run', kind: 'transcript', id: 'a', text: 'Earlier speech', done: true });
  const next = createSession('Session 2');
  assert.notEqual(first.id, next.id); assert.equal(next.context, ''); assert.equal(next.turns.length, 0);
  assert.equal(first.turns[0].text, 'Earlier speech');
});

test('pairs answers with meeting turns and ignores microphone suggestions', () => {
  let turns = updateTurns([], { sessionId: 's', source: 'meeting', kind: 'transcript', id: 'same', text: 'Question', done: true });
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'transcript', id: 'same', text: 'My reply', done: true });
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'reply', id: 'mic-response', turnId: 'same', text: 'Next ' });
  turns = updateTurns(turns, { sessionId: 's', source: 'meeting', kind: 'reply', id: 'remote-response', turnId: 'same', text: 'Answer', done: true });
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'reply', id: 'mic-response', text: 'step' });
  assert.equal(turns[0].reply?.text, 'Answer'); assert.equal(turns[1].reply, undefined); assert.equal(turns[1].text, 'My reply');
});

test('final transcripts replace partials and cancelled replies reject late deltas', () => {
  let turns = updateTurns([], { sessionId: 's', kind: 'transcript', id: 'a', text: 'Hel' });
  turns = updateTurns(turns, { sessionId: 's', kind: 'transcript', id: 'b', text: 'Next' });
  turns = updateTurns(turns, { sessionId: 's', kind: 'transcript', id: 'a', text: 'Hello.', done: true });
  turns = updateTurns(turns, { sessionId: 's', kind: 'reply', id: 'r', turnId: 'a', text: 'I suggest' });
  turns = updateTurns(turns, { sessionId: 's', kind: 'reply_end', id: 'r', status: 'cancelled' });
  turns = updateTurns(turns, { sessionId: 's', kind: 'reply', id: 'r', text: ' stale' });
  assert.deepEqual(turns.map(t => t.text), ['Hello.', 'Next']);
  assert.equal(turns[0].reply?.text, 'I suggest'); assert.equal(turns[0].reply?.interrupted, true);
});

test('interim transcription corrections replace hypotheses and preserve the final transcript', () => {
  let turns = updateTurns([], { sessionId: 's', source: 'microphone', kind: 'transcript', id: 'mic', text: 'I can', replace: true });
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'transcript', id: 'mic', text: 'I will check', replace: true });
  assert.equal(turns[0].text, 'I will check');
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'transcript', id: 'mic', text: 'I will check with logistics.', done: true });
  turns = updateTurns(turns, { sessionId: 's', source: 'microphone', kind: 'transcript', id: 'mic', text: 'stale', replace: true });
  assert.equal(turns[0].text, 'I will check with logistics.');
});

test('exports both transcript sources and meeting answers without private context', () => {
  const session = createSession('Session 1'); session.context = 'Secret context not requested in export';
  for (let i = 0; i < 100; i++) session.turns = updateTurns(session.turns, {
    sessionId: 's', source: i % 2 ? 'microphone' : 'meeting', kind: 'transcript', id: String(i), text: `Speech ${i}`, done: true,
  });
  session.turns = updateTurns(session.turns, { sessionId: 's', source: 'meeting', kind: 'reply', id: 'r', turnId: '98', text: '<script>next</script>', done: true });
  const markdown = exportSession(session);
  assert.equal(session.turns.length, 100); assert(markdown.includes('Speech 0')); assert(markdown.includes('Speech 99'));
  assert(markdown.includes('Your microphone')); assert(markdown.includes('AI suggested answer'));
  assert(!markdown.includes(session.context)); assert(!markdown.includes('<script>')); assert(markdown.includes('&lt;script&gt;'));
});

test('marks unfinished recording data and ignores orphan replies', () => {
  const turns = updateTurns([], { sessionId: 's', kind: 'transcript', id: 'a', text: 'Partial' });
  assert(finishTurns(turns)[0].incomplete);
  assert.deepEqual(updateTurns([], { sessionId: 's', kind: 'reply', id: 'missing', text: 'Orphan' }), []);
});

test('resuming a session retains earlier turns even when the provider reuses item IDs', () => {
  let turns = updateTurns([], { sessionId: 'first-run', kind: 'transcript', id: 'item', text: 'First segment', done: true });
  turns = updateTurns(turns, { sessionId: 'resumed-run', kind: 'transcript', id: 'item', text: 'Second segment', done: true });
  assert.deepEqual(turns.map(t => t.text), ['First segment', 'Second segment']);
});
