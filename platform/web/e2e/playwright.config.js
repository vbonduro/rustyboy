import { defineConfig } from '@playwright/test';
import http from 'http';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const STATIC_DIR = path.resolve(__dirname, '../client/static');
const FIXTURES_DIR = path.resolve(__dirname, 'fixtures');
const PORT = 3737;

const MIME = {
  '.html': 'text/html',
  '.js':   'application/javascript',
  '.css':  'text/css',
  '.png':  'image/png',
  '.wasm': 'application/wasm',
};

function createServer() {
  return http.createServer((req, res) => {
    const url = req.url.split('?')[0];

    // Serve stub WASM module for the real emulator JS
    if (url === '/static/rustyboy_web_client.js') {
      const stubPath = path.join(FIXTURES_DIR, 'rustyboy_web_client_stub.js');
      const content = fs.readFileSync(stubPath, 'utf8');
      res.writeHead(200, { 'Content-Type': 'application/javascript' });
      res.end(content);
      return;
    }

    // Serve test fixture pages
    if (url === '/test/menu') {
      const fixturePath = path.join(FIXTURES_DIR, 'menu_test.html');
      const content = fs.readFileSync(fixturePath, 'utf8');
      res.writeHead(200, { 'Content-Type': 'text/html' });
      res.end(content);
      return;
    }

    // Serve static files from client/static/
    if (url.startsWith('/static/')) {
      const filePath = path.join(STATIC_DIR, url.slice('/static/'.length));
      if (fs.existsSync(filePath) && fs.statSync(filePath).isFile()) {
        const ext = path.extname(filePath);
        const mime = MIME[ext] || 'application/octet-stream';
        res.writeHead(200, { 'Content-Type': mime });
        res.end(fs.readFileSync(filePath));
        return;
      }
    }

    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('Not found');
  });
}

// Start and stop the server around test runs via globalSetup/globalTeardown
// We use webServer with a command that keeps the server running.
// Playwright's webServer.command approach: we write a small server script.

export default defineConfig({
  testDir: './tests',
  use: {
    baseURL: `http://localhost:${PORT}`,
  },
  projects: [
    {
      name: 'chromium',
      use: { browserName: 'chromium' },
    },
  ],
  webServer: {
    command: `node ${path.resolve(__dirname, 'server.cjs')}`,
    url: `http://localhost:${PORT}/test/menu`,
    reuseExistingServer: true,
    timeout: 10000,
  },
});
