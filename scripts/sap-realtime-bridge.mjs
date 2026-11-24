import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export function createBridge({ aiCore, keys, validateApiKey, WebSocket, WebSocketServer }) {
  const server = createServer((req, res) => {
    res.writeHead(req.url === '/healthz' ? 200 : 404, { 'Content-Type': 'text/plain' });
    res.end(req.url === '/healthz' ? 'OK' : 'Use /v1/realtime over WebSocket.');
  });
  const wss = new WebSocketServer({ noServer: true, maxPayload: 1024 * 1024, perMessageDeflate: false });
  const connections = new Set();
  const reject = (socket, status) => socket.end(`HTTP/1.1 ${status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  server.on('upgrade', async (req, socket, head) => {
    socket.on('error', () => socket.destroy());
    const url = new URL(req.url, 'http://localhost');
    if (url.pathname !== '/v1/realtime') return reject(socket, '404 Not Found');
    const bearer = req.headers.authorization?.startsWith('Bearer ') ? req.headers.authorization.slice(7) : undefined;
    const key = bearer || req.headers['x-api-key'] || req.headers['api-key'];
    // The bridge always requires a key, even when the original proxy disables authentication locally.
    if (!keys.length || !validateApiKey(key, keys)) return reject(socket, '401 Unauthorized');
    if (req.headers.origin) return reject(socket, '403 Forbidden');
    if (url.searchParams.get('model') !== 'gpt-realtime') return reject(socket, '400 Bad Request');
    if (connections.size >= 4) return reject(socket, '429 Too Many Requests');
    connections.add(socket);
    let upstream;
    let client;
    let pending = [];
    const cleanup = () => {
      connections.delete(socket);
      if (upstream && upstream.readyState !== WebSocket.CLOSED) upstream.terminate();
    };
    socket.once('close', cleanup);
    const timer = setTimeout(() => { reject(socket, '504 Gateway Timeout'); cleanup(); }, 20000);
    try {
      const deployment = await aiCore.deploymentCatalog.findDeploymentFor('gpt-realtime');
      if (!deployment) { clearTimeout(timer); return reject(socket, '404 Not Found'); }
      const token = await aiCore.tokenProvider.getValidToken();
      if (socket.destroyed) { clearTimeout(timer); return; }
      const target = new URL(`${aiCore.config.aiCore.baseUrl}/v2/inference/deployments/${encodeURIComponent(deployment.id)}/v1/realtime`);
      target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:';
      target.searchParams.set('model', 'gpt-realtime');
      upstream = new WebSocket(target, {
        headers: { Authorization: `Bearer ${token}`, 'AI-Resource-Group': aiCore.config.aiCore.resourceGroup, 'AI-Client-Type': 'Meeting_Copilot' },
        handshakeTimeout: 15000, maxPayload: 1024 * 1024, perMessageDeflate: false,
      });
      const forward = (destination, data, binary) => {
        if (destination.readyState !== WebSocket.OPEN) return;
        if (destination.bufferedAmount > 2 * 1024 * 1024) { client?.close(1013, 'Connection too slow'); cleanup(); return; }
        destination.send(data, { binary }, error => { if (error) { client?.close(1011, 'Connection interrupted'); cleanup(); } });
      };
      upstream.on('message', (data, binary) => {
        if (client) forward(client, data, binary);
        else if (pending.length < 4) pending.push([data, binary]);
        else cleanup();
      });
      upstream.once('open', () => {
        clearTimeout(timer);
        if (socket.destroyed) return cleanup();
        wss.handleUpgrade(req, socket, head, ws => {
          client = ws;
          ws.on('message', (data, binary) => forward(upstream, data, binary));
          ws.on('error', cleanup);
          ws.on('close', cleanup);
          for (const [data, binary] of pending) forward(ws, data, binary);
          pending = [];
        });
      });
      upstream.on('error', () => {
        clearTimeout(timer);
        if (client) client.close(1011, 'AI Core Realtime connection failed');
        else reject(socket, '502 Bad Gateway');
        cleanup();
      });
      upstream.on('unexpected-response', (_request, response) => {
        clearTimeout(timer);
        response.resume();
        reject(socket, response.statusCode === 503 ? '503 Service Unavailable' : '502 Bad Gateway');
        cleanup();
      });
      upstream.on('close', () => {
        clearTimeout(timer);
        if (client) client.close(1000, 'AI Core Realtime session ended');
        else if (!socket.destroyed) reject(socket, '502 Bad Gateway');
      });
    } catch {
      clearTimeout(timer);
      reject(socket, '502 Bad Gateway');
      cleanup();
    }
  });
  server.on('close', () => { for (const socket of connections) socket.destroy(); wss.close(); });
  return server;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const proxyPath = resolve(process.argv[2] || '/home/hanz/sap-ai-proxy');
  const fromProxy = path => import(pathToFileURL(resolve(proxyPath, path)).href);
  const { loadEnvFromPath } = await fromProxy('src/index.js');
  loadEnvFromPath(resolve(proxyPath, '.env'));
  const { getConfig } = await fromProxy('src/lib/config.js');
  const { AICore } = await fromProxy('src/lib/ai-core/index.js');
  const { getApiKeys } = await fromProxy('src/lib/api-key-config.js');
  const { validateApiKey } = await fromProxy('src/lib/auth.js');
  const { WebSocket, WebSocketServer } = createRequire(resolve(proxyPath, 'package.json'))('ws');
  const keys = process.env.MEETING_PROXY_API_KEY ? [process.env.MEETING_PROXY_API_KEY] : getApiKeys();
  const server = createBridge({ aiCore: new AICore(getConfig()), keys, validateApiKey, WebSocket, WebSocketServer });
  server.on('error', error => { console.error(`Realtime bridge could not listen: ${error.code || 'server error'}`); process.exitCode = 1; });
  server.listen(8081, '127.0.0.1', () => console.log('Realtime bridge ready at ws://localhost:8081/v1/realtime?model=gpt-realtime'));
  const stop = () => { server.closeAllConnections(); server.close(); };
  process.on('SIGTERM', stop);
  process.on('SIGINT', stop);
}
