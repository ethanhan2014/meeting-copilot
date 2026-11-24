import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { createBridge } from './sap-realtime-bridge.mjs';

const { WebSocket, WebSocketServer } = createRequire('/home/hanz/sap-ai-proxy/package.json')('ws');
test('bridge authenticates before contacting AI Core and preserves text frames in both directions', async () => {
  const upstream = new WebSocketServer({ port: 0 });
  await once(upstream, 'listening');
  let lookups = 0;
  upstream.on('connection', (ws, req) => {
    assert.equal(req.url, '/v2/inference/deployments/test/v1/realtime?model=gpt-realtime');
    assert.equal(req.headers.authorization, 'Bearer upstream-token');
    assert.equal(req.headers['x-api-key'], undefined);
    ws.send(JSON.stringify({ type: 'session.created' }));
    ws.on('message', (message, binary) => { assert.equal(binary, false); ws.send(message.toString()); });
  });
  const server = createBridge({
    aiCore: { config: { aiCore: { baseUrl: `http://127.0.0.1:${upstream.address().port}`, resourceGroup: 'default' } },
      deploymentCatalog: { findDeploymentFor: async () => { lookups++; return { id: 'test' }; } },
      tokenProvider: { getValidToken: async () => 'upstream-token' } },
    keys: ['local-key'], validateApiKey: (key, keys) => keys.includes(key), WebSocket, WebSocketServer,
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const endpoint = `ws://127.0.0.1:${server.address().port}/v1/realtime?model=gpt-realtime`;
  try {
    const invalid = new WebSocket(endpoint, { headers: { 'x-api-key': 'wrong' } });
    const [error] = await once(invalid, 'error');
    assert.match(error.message, /401/); assert.equal(lookups, 0);
    const ws = new WebSocket(endpoint, { headers: { 'x-api-key': 'local-key' } });
    const received = [];
    ws.on('message', (message, binary) => { assert.equal(binary, false); received.push(JSON.parse(message.toString())); });
    await once(ws, 'open');
    const message = once(ws, 'message');
    ws.send(JSON.stringify({ type: 'session.update', session: { output_modalities: ['text'] } }));
    await message;
    if (received.length < 2) await once(ws, 'message');
    assert.equal(received[0].type, 'session.created');
    assert.equal(received[1].type, 'session.update');
    ws.close(); await once(ws, 'close');
  } finally { server.close(); upstream.close(); }
});

test('reports upstream service unavailability as HTTP 503', async () => {
  const upstream = createServer();
  upstream.on('upgrade', (_req, socket) => socket.end('HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'));
  upstream.listen(0, '127.0.0.1'); await once(upstream, 'listening');
  const server = createBridge({
    aiCore: { config: { aiCore: { baseUrl: `http://127.0.0.1:${upstream.address().port}`, resourceGroup: 'default' } },
      deploymentCatalog: { findDeploymentFor: async () => ({ id: 'test' }) },
      tokenProvider: { getValidToken: async () => 'upstream-token' } },
    keys: ['local-key'], validateApiKey: (key, keys) => keys.includes(key), WebSocket, WebSocketServer,
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  try {
    const ws = new WebSocket(`ws://127.0.0.1:${server.address().port}/v1/realtime?model=gpt-realtime`, { headers: { 'api-key': 'local-key' } });
    const [error] = await once(ws, 'error');
    assert.match(error.message, /503/);
  } finally { server.close(); upstream.close(); }
});
