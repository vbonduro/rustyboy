const http = require('http');
const fs   = require('fs');
const path = require('path');

const STATIC_DIR   = path.resolve(__dirname, '../client/static');
const FIXTURES_DIR = path.resolve(__dirname, 'fixtures');
const PORT = 3737;

const MIME = {
  '.html': 'text/html',
  '.js':   'application/javascript',
  '.css':  'text/css',
  '.png':  'image/png',
  '.wasm': 'application/wasm',
};

// Test state controlled by the test suite via POST /test/control
let mockState = {
  authed:     false,   // whether /api/me returns 200 or 401
  roms:       ['Tetris.gb', 'Mario.gb', 'Zelda.gb'],
  saveStates: [],      // array of {id, rom_name, slot_name, updated_at}
};

http.createServer((req, res) => {
  const url = req.url.split('?')[0];

  // ── Test control endpoint ──────────────────────────────────────────────────
  if (req.method === 'POST' && url === '/test/control') {
    let body = '';
    req.on('data', d => { body += d; });
    req.on('end', () => {
      try { Object.assign(mockState, JSON.parse(body)); } catch (_) {}
      res.writeHead(200);
      res.end('ok');
    });
    return;
  }

  // ── Mock API endpoints ─────────────────────────────────────────────────────
  if (url === '/api/me') {
    if (mockState.authed) {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ id: 'test-user', display_name: 'Test User', email: 'test@test.com', avatar_url: null }));
    } else {
      res.writeHead(401, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ error: 'unauthorized' }));
    }
    return;
  }

  if (url === '/api/roms') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(mockState.roms));
    return;
  }

  // /auth/google: simulate instant login by setting authed and redirecting back
  if (url === '/auth/google') {
    mockState.authed = true;
    res.writeHead(302, { 'Location': '/test/app' });
    res.end();
    return;
  }

  // /auth/logout: clear auth, redirect to login
  if (req.method === 'POST' && url === '/auth/logout') {
    mockState.authed = false;
    res.writeHead(200);
    res.end('ok');
    return;
  }

  // /api/auth-method: always return google
  if (url === '/api/auth-method') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ methods: ['google'] }));
    return;
  }

  // /api/save-states — list roms with saves
  if (req.method === 'GET' && url === '/api/save-states') {
    const romsWithSaves = [...new Set(mockState.saveStates.map(s => s.rom_name))]
      .map(rom => {
        const latest = mockState.saveStates
          .filter(s => s.rom_name === rom)
          .sort((a, b) => b.updated_at - a.updated_at)[0];
        return { rom_name: rom, last_saved: latest.updated_at };
      })
      .sort((a, b) => b.last_saved - a.last_saved);
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(romsWithSaves));
    return;
  }

  // /api/save-states/:rom/latest
  const latestMatch = url.match(/^\/api\/save-states\/([^/]+)\/latest$/);
  if (req.method === 'GET' && latestMatch) {
    const rom = decodeURIComponent(latestMatch[1]);
    const saves = mockState.saveStates
      .filter(s => s.rom_name === rom)
      .sort((a, b) => b.updated_at - a.updated_at);
    if (saves.length === 0) {
      res.writeHead(404); res.end(); return;
    }
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify(saves[0]));
    return;
  }

  // /api/save-states/:rom — list slots or POST new save
  const romSavesMatch = url.match(/^\/api\/save-states\/([^/]+)$/);
  if (romSavesMatch) {
    const rom = decodeURIComponent(romSavesMatch[1]);
    if (req.method === 'GET') {
      const saves = mockState.saveStates
        .filter(s => s.rom_name === rom)
        .sort((a, b) => b.updated_at - a.updated_at)
        .map(({ id, slot_name, updated_at }) => ({ id, slot_name, updated_at }));
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(saves));
      return;
    }
    if (req.method === 'POST') {
      const id = `mock-ss-${Date.now()}`;
      const slot_name = String(Date.now());
      const updated_at = Math.floor(Date.now() / 1000);
      mockState.saveStates.push({ id, rom_name: rom, slot_name, updated_at });
      res.writeHead(201, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ id, slot_name, updated_at }));
      return;
    }
  }

  // /api/save-states/by-id/:id — DELETE
  const deleteMatch = url.match(/^\/api\/save-states\/by-id\/([^/]+)$/);
  if (req.method === 'DELETE' && deleteMatch) {
    const id = deleteMatch[1];
    mockState.saveStates = mockState.saveStates.filter(s => s.id !== id);
    res.writeHead(204); res.end(); return;
  }

  // /api/save-states/by-id/:id/data — download blob
  const dataMatch = url.match(/^\/api\/save-states\/by-id\/([^/]+)\/data$/);
  if (req.method === 'GET' && dataMatch) {
    const id = dataMatch[1];
    const ss = mockState.saveStates.find(s => s.id === id);
    if (!ss) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'Content-Type': 'application/octet-stream' });
    res.end(Buffer.alloc(16)); // fake blob
    return;
  }

  // /api/battery-saves/:rom — stub (always 404 for get, 204 for put)
  if (url.startsWith('/api/battery-saves/')) {
    if (req.method === 'GET') { res.writeHead(404); res.end(); return; }
    if (req.method === 'PUT') { res.writeHead(204); res.end(); return; }
  }

  // /dev/log
  if (req.method === 'POST' && url === '/dev/log') {
    res.writeHead(204); res.end(); return;
  }

  // Mock ROM serving — return fake bytes
  if (url.startsWith('/roms/')) {
    res.writeHead(200, { 'Content-Type': 'application/octet-stream' });
    res.end(Buffer.alloc(32768)); // 32kb of zeros — valid enough for stub emulator
    return;
  }

  // ── Fixture pages ──────────────────────────────────────────────────────────
  if (url === '/static/rustyboy_web_client.js') {
    const c = fs.readFileSync(path.join(FIXTURES_DIR, 'rustyboy_web_client_stub.js'), 'utf8');
    res.writeHead(200, { 'Content-Type': 'application/javascript' });
    res.end(c);
    return;
  }

  if (url === '/test/menu') {
    const c = fs.readFileSync(path.join(FIXTURES_DIR, 'menu_test.html'), 'utf8');
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(c);
    return;
  }

  if (url === '/test/app' || url === '/') {
    const c = fs.readFileSync(path.join(FIXTURES_DIR, 'app_test.html'), 'utf8');
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(c);
    return;
  }

  if (url.startsWith('/static/')) {
    const fp = path.join(STATIC_DIR, url.slice('/static/'.length));
    if (fs.existsSync(fp) && fs.statSync(fp).isFile()) {
      const ext = path.extname(fp);
      res.writeHead(200, { 'Content-Type': MIME[ext] || 'application/octet-stream' });
      res.end(fs.readFileSync(fp));
      return;
    }
  }

  res.writeHead(404);
  res.end('Not found');
}).listen(PORT, () => console.log(`server ready on ${PORT}`));
