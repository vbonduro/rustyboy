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
  authed: false,   // whether /api/me returns 200 or 401
  roms:   ['Tetris.gb', 'Mario.gb', 'Zelda.gb'],
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
