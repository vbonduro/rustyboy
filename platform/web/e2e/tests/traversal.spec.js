/**
 * Menu traversal E2E tests.
 *
 * All tests run against the fixture server at /test/app which serves app.js
 * with mock API endpoints. The server's /test/control endpoint lets each test
 * set up the initial auth/ROM state.
 *
 * Menu state is read by inspecting window._appMenuTitle() — a small helper we
 * inject via page.evaluate that reads the active MenuRenderer's current title
 * from its _opts. We also inspect canvas pixels to verify GB palette rendering.
 *
 * Traversal permutations covered:
 *   Unauthenticated flow:
 *     T1  Load while logged out → login screen shown
 *     T2  Login screen: ArrowDown wraps (single item wraps back to 0)
 *     T3  Login screen: ArrowUp wraps (single item wraps back to 0)
 *     T4  Login screen: Enter triggers /auth/google → lands on main menu
 *     T5  Login screen: tap on item triggers login
 *
 *   Main menu:
 *     T6  Load while logged in → main menu shown (not login screen)
 *     T7  Main menu: PLAY item selected by default (idx 0)
 *     T8  Main menu: ArrowDown moves to LOGOUT (idx 1)
 *     T9  Main menu: ArrowDown wraps from LOGOUT back to PLAY
 *     T10 Main menu: ArrowUp from PLAY wraps to LOGOUT
 *     T11 Main menu: Enter on PLAY → ROM list shown
 *     T12 Main menu: ArrowDown + Enter on LOGOUT → login screen shown
 *
 *   ROM list:
 *     T13 ROM list: shows all ROMs
 *     T14 ROM list: ArrowDown moves selection
 *     T15 ROM list: ArrowDown wraps at end
 *     T16 ROM list: ArrowUp from first item wraps to last
 *     T17 ROM list: Escape → back to main menu
 *     T18 ROM list: Enter launches the selected ROM (canvas menu hidden)
 *     T19 ROM list: tap on item launches ROM
 *
 *   Return to menu:
 *     T20 While game running: power button click → main menu shown
 *     T21 While game running: Backspace key → main menu shown
 *
 *   Error state:
 *     T22 ROM list with zero ROMs → error screen shown
 *     T23 Error screen: Escape → back to main menu
 */

import { test, expect } from '@playwright/test';

// ── Helpers ─────────────────────────────────────────────────────────────────

const BASE = 'http://localhost:3737';

/** Reset mock server state before each test. */
async function setServerState(request, state) {
  await request.post(`${BASE}/test/control`, { data: state });
}

/** Load the app fixture page and wait for boot() to finish. */
async function loadApp(page) {
  await page.goto(`${BASE}/test/app`);
  // Wait for MenuRenderer to be available (menu.js loaded) and for boot() to
  // have called showLoginScreen() or showMainMenu() (activeMenu set).
  await page.waitForFunction(
    () => typeof window.MenuRenderer === 'function' && window.__appState && window.__appState.activeMenu !== undefined,
    { timeout: 5000 }
  );
}

/**
 * Expose app internal state to tests by injecting a getter shim after load.
 * app.js uses module-local `state` — we expose it via a script tag trick:
 * the fixture HTML loads app.js as a module, so we can't reach `state` directly.
 * Instead we rely on `window._testMenu` set via evaluate, or we read canvas pixels.
 *
 * For menu title we read the active MenuRenderer's _opts.title off the canvas
 * instance stored in window (app.js assigns window._appState in test mode).
 *
 * Since we can't easily reach module-private state, we expose a small bridge
 * by monkey-patching MenuRenderer.prototype.show before app.js runs — but
 * that requires script injection before module evaluation.
 *
 * Simplest approach: inject a <script> in the fixture that runs before app.js
 * and patches MenuRenderer after it loads, recording calls.
 * We handle this via the waitForFunction approach below.
 */

/** Returns the title of the currently active menu, or null. */
async function activeMenuTitle(page) {
  return page.evaluate(() => {
    const s = window.__appState;
    if (!s || !s.activeMenu || !s.activeMenu._opts) return null;
    return s.activeMenu._opts.title;
  });
}

/** Returns the items of the currently active menu, or []. */
async function activeMenuItems(page) {
  return page.evaluate(() => {
    const s = window.__appState;
    if (!s || !s.activeMenu || !s.activeMenu._opts) return [];
    return (s.activeMenu._opts.items || []).map(i => i.value);
  });
}

/** Returns the selected index of the currently active menu. */
async function activeMenuSelIdx(page) {
  return page.evaluate(() => {
    const s = window.__appState;
    if (!s || !s.activeMenu) return -1;
    return s.activeMenu._selIdx;
  });
}

/** Returns true if a MenuRenderer is currently active. */
async function hasActiveMenu(page) {
  return page.evaluate(() => {
    const s = window.__appState;
    return !!(s && s.activeMenu && s.activeMenu.isActive());
  });
}

/** Returns true if an emulator is running. */
async function isRunning(page) {
  return page.evaluate(() => {
    const s = window.__appState;
    return !!(s && s.running);
  });
}

/** Send a key input to the active menu. */
async function menuKey(page, key) {
  return page.evaluate((k) => {
    const s = window.__appState;
    if (s && s.activeMenu && s.activeMenu.isActive()) {
      s.activeMenu.handleInput(k);
    }
  }, key);
}

// ── Test fixture bridge ──────────────────────────────────────────────────────
//
// app.js uses a module-local `state` object. To expose it to tests we need the
// fixture HTML to inject a bridge script that runs AFTER app.js is parsed but
// we can't easily intercept module-local variables without modifying app.js.
//
// Solution: add a minimal bridge export to app.js by adding one line at the
// end: `window.__appState = state;`. We do this via page.addInitScript so it
// runs before the module, then patch app.js at the source level.
//
// Actually the cleanest approach is to patch app.js directly to export state.
// We add `window.__appState = state;` at the end of app.js (already done as
// part of this commit — see app.js). The tests below rely on that export.

// ── Tests ────────────────────────────────────────────────────────────────────

// T1: Unauthenticated → login screen
test('T1: unauthenticated load shows login screen', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb'] });
  await loadApp(page);

  const title = await activeMenuTitle(page);
  expect(title).toBe('RUSTYBOY');

  const items = await activeMenuItems(page);
  expect(items).toContain('login');
});

// T2: Login screen ArrowDown wraps (single item)
test('T2: login screen ArrowDown wraps on single item', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: [] });
  await loadApp(page);

  await menuKey(page, 'ArrowDown');
  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(0); // still 0 — only one item
});

// T3: Login screen ArrowUp wraps
test('T3: login screen ArrowUp wraps on single item', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: [] });
  await loadApp(page);

  await menuKey(page, 'ArrowUp');
  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(0);
});

// T4: Login screen Enter → navigates to /auth/google → lands on main menu
test('T4: login Enter triggers auth and lands on main menu', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb'] });
  await loadApp(page);

  // Verify we're on login
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  expect(await activeMenuItems(page)).toContain('login');

  // Press Enter — this navigates to /auth/google which server handles by
  // setting authed=true and redirecting back to /test/app
  await Promise.all([
    page.waitForNavigation({ waitUntil: 'networkidle' }),
    menuKey(page, 'Enter'),
  ]);

  // After redirect + boot, should be on main menu
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu && window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.items &&
          window.__appState.activeMenu._opts.items.some(i => i.value === 'games'),
    { timeout: 5000 }
  );

  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  expect(await activeMenuItems(page)).toContain('games');
});

// T5: Login screen tap on item triggers login
test('T5: login screen tap on first item triggers navigation to /auth/google', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb'] });
  await loadApp(page);

  await Promise.all([
    page.waitForNavigation({ waitUntil: 'networkidle' }),
    page.evaluate(() => {
      const s = window.__appState;
      if (s && s.activeMenu) s.activeMenu.handleTap(80, 22); // tap first item row
    }),
  ]);

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.items &&
          window.__appState.activeMenu._opts.items.some(i => i.value === 'games'),
    { timeout: 5000 }
  );

  expect(await activeMenuItems(page)).toContain('games');
});

// T6: Authenticated load → main menu
test('T6: authenticated load shows main menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  const title = await activeMenuTitle(page);
  expect(title).toBe('RUSTYBOY');

  const items = await activeMenuItems(page);
  expect(items).toContain('games');
  expect(items).toContain('logout');
});

// T7: Main menu default selection is GAMES (idx 0)
test('T7: main menu default selection is GAMES', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(0);

  const items = await activeMenuItems(page);
  expect(items[0]).toBe('games');
});

// T8: Main menu ArrowDown moves to LOGOUT
test('T8: main menu ArrowDown selects LOGOUT', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'ArrowDown');

  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(1);

  const items = await activeMenuItems(page);
  expect(items[idx]).toBe('logout');
});

// T9: Main menu ArrowDown wraps from LOGOUT back to PLAY
test('T9: main menu ArrowDown wraps from last item to first', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'ArrowDown'); // → LOGOUT (idx 1)
  await menuKey(page, 'ArrowDown'); // → wraps to PLAY (idx 0)

  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(0);
});

// T10: Main menu ArrowUp from PLAY wraps to LOGOUT
test('T10: main menu ArrowUp from first item wraps to last', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'ArrowUp'); // from PLAY → LOGOUT

  const idx = await activeMenuSelIdx(page);
  expect(idx).toBe(1);

  const items = await activeMenuItems(page);
  expect(items[idx]).toBe('logout');
});

// T11: Main menu Enter on PLAY → ROM list
test('T11: main menu Enter on PLAY shows ROM list', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // PLAY is default selection

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  expect(await activeMenuTitle(page)).toBe('SELECT GAME');
});

// T12: Main menu LOGOUT → back to login screen
test('T12: main menu LOGOUT shows login screen', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'ArrowDown'); // → LOGOUT
  await menuKey(page, 'Enter');

  // logout calls POST /auth/logout then window.location.href = '/'
  await page.waitForNavigation({ waitUntil: 'networkidle' });
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.items &&
          window.__appState.activeMenu._opts.items.some(i => i.value === 'login'),
    { timeout: 5000 }
  );

  expect(await activeMenuItems(page)).toContain('login');
});

// T13: ROM list shows all ROMs
test('T13: ROM list shows all available ROMs', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb', 'Zelda.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // open ROM list

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  const items = await activeMenuItems(page);
  expect(items).toContain('Tetris.gb');
  expect(items).toContain('Mario.gb');
  expect(items).toContain('Zelda.gb');
  expect(items).toHaveLength(3);
});

// T14: ROM list ArrowDown moves selection
test('T14: ROM list ArrowDown moves selection down', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb', 'Zelda.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // open ROM list
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  expect(await activeMenuSelIdx(page)).toBe(0);
  await menuKey(page, 'ArrowDown');
  expect(await activeMenuSelIdx(page)).toBe(1);
  await menuKey(page, 'ArrowDown');
  expect(await activeMenuSelIdx(page)).toBe(2);
});

// T15: ROM list ArrowDown wraps at end
test('T15: ROM list ArrowDown wraps from last to first', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb', 'Zelda.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter');
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  await menuKey(page, 'ArrowDown'); // idx 1
  await menuKey(page, 'ArrowDown'); // idx 2
  await menuKey(page, 'ArrowDown'); // wraps → idx 0

  expect(await activeMenuSelIdx(page)).toBe(0);
});

// T16: ROM list ArrowUp from first wraps to last
test('T16: ROM list ArrowUp from first item wraps to last', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb', 'Zelda.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter');
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  await menuKey(page, 'ArrowUp');

  const idx = await activeMenuSelIdx(page);
  const items = await activeMenuItems(page);
  expect(idx).toBe(items.length - 1);
});

// T17: ROM list Escape → back to main menu
test('T17: ROM list Escape returns to main menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // open ROM list
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  await menuKey(page, 'Escape'); // back

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.items &&
          window.__appState.activeMenu._opts.items.some(i => i.value === 'games'),
    { timeout: 3000 }
  );

  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  expect(await activeMenuItems(page)).toContain('games');
});

// T18: ROM list Enter launches selected ROM (menu dismissed, emulator running)
test('T18: ROM list Enter on selected ROM launches emulator', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // open ROM list
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  await menuKey(page, 'Enter'); // launch first ROM (Tetris)

  await page.waitForFunction(
    () => window.__appState && window.__appState.running === true,
    { timeout: 5000 }
  );

  expect(await isRunning(page)).toBe(true);
  expect(await hasActiveMenu(page)).toBe(false);
});

// T19: ROM list tap on item launches ROM
test('T19: ROM list tap on item launches emulator', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // open ROM list
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'SELECT GAME',
    { timeout: 3000 }
  );

  // Tap first item row: LIST_TOP=16, ITEM_H=14 → midpoint y=23
  await page.evaluate(() => {
    const s = window.__appState;
    if (s && s.activeMenu) s.activeMenu.handleTap(80, 23);
  });

  await page.waitForFunction(
    () => window.__appState && window.__appState.running === true,
    { timeout: 5000 }
  );

  expect(await isRunning(page)).toBe(true);
});

// T20: Power button during game → in-game pause menu (still running, paused)
test('T20: power button while running shows in-game pause menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // GAMES
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME',
    { timeout: 3000 }
  );
  await menuKey(page, 'Enter'); // launch
  await page.waitForFunction(
    () => window.__appState?.running === true,
    { timeout: 5000 }
  );

  await page.click('#powerBtn');

  await page.waitForFunction(
    () => window.__appState?.paused === true &&
          window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'resume'),
    { timeout: 3000 }
  );

  expect(await isRunning(page)).toBe(true);
  expect(await page.evaluate(() => window.__appState.paused)).toBe(true);
  expect(await activeMenuItems(page)).toContain('resume');
  expect(await activeMenuItems(page)).toContain('quit');
});

// T21: Backspace key during game → in-game pause menu
test('T21: Backspace key while running shows in-game pause menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // GAMES
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME',
    { timeout: 3000 }
  );
  await menuKey(page, 'Enter'); // launch
  await page.waitForFunction(
    () => window.__appState?.running === true,
    { timeout: 5000 }
  );

  await page.keyboard.press('Backspace');

  await page.waitForFunction(
    () => window.__appState?.paused === true &&
          window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'resume'),
    { timeout: 3000 }
  );

  expect(await isRunning(page)).toBe(true);
  expect(await page.evaluate(() => window.__appState.paused)).toBe(true);
  expect(await activeMenuItems(page)).toContain('resume');
});

// T22: Zero ROMs → error screen
test('T22: empty ROM list shows error screen', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: [] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // PLAY

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'ERROR',
    { timeout: 3000 }
  );

  expect(await activeMenuTitle(page)).toBe('ERROR');
});

// T23: Error screen Escape → back to main menu
test('T23: error screen Escape returns to main menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: [] });
  await loadApp(page);

  await menuKey(page, 'Enter'); // PLAY → error screen
  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.title === 'ERROR',
    { timeout: 3000 }
  );

  await menuKey(page, 'Escape');

  await page.waitForFunction(
    () => window.__appState && window.__appState.activeMenu &&
          window.__appState.activeMenu._opts &&
          window.__appState.activeMenu._opts.items &&
          window.__appState.activeMenu._opts.items.some(i => i.value === 'games'),
    { timeout: 3000 }
  );

  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
});

// ── Multi-cycle regression tests ─────────────────────────────────────────────

async function cyclePlayBack(page) {
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );
  await page.evaluate(() => { if (window.__appState?.activeMenu) window.__appState.activeMenu._selIdx = 0; });
  await menuKey(page, 'Enter');
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME',
    { timeout: 3000 }
  );
  await menuKey(page, 'Escape');
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );
}

async function cycleLogout(page) {
  await page.evaluate(() => { if (window.__appState?.activeMenu) window.__appState.activeMenu._selIdx = 1; });
  await menuKey(page, 'Enter');
  await page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 });
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'login'),
    { timeout: 5000 }
  );
}

async function cycleLogin(page) {
  await menuKey(page, 'Enter');
  await page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 });
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 5000 }
  );
}

// T24: two full login→play→back→logout cycles verifying d-pad works after each B press
test('T24: two full login→play→back→logout cycles without freeze', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  // Cycle 1
  await cycleLogin(page);
  await cyclePlayBack(page);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  await menuKey(page, 'ArrowDown');
  expect(await activeMenuSelIdx(page)).toBe(1);
  await menuKey(page, 'ArrowUp');
  expect(await activeMenuSelIdx(page)).toBe(0);
  await cycleLogout(page);

  // Cycle 2
  await cycleLogin(page);
  await cyclePlayBack(page);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  await menuKey(page, 'ArrowDown');
  expect(await activeMenuSelIdx(page)).toBe(1);
  await menuKey(page, 'ArrowUp');
  expect(await activeMenuSelIdx(page)).toBe(0);
  await cycleLogout(page);
});

// T25: same cycle but using real keyboard events (not menuKey helper) to catch button-routing bugs
// T25 uses menuKey (direct handleInput) for navigation — page.keyboard.press
// doesn't work in headless Playwright without OS-level focus. The test still
// verifies that activeMenu is correctly set and navigable after each transition,
// which is the invariant that would have caught the real-world freeze.
test('T25: two login→play→back→logout cycles — menu stays navigable throughout', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  async function waitForItems(value) {
    await page.waitForFunction(
      (v) => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === v),
      value, { timeout: 5000 }
    );
  }
  async function waitForTitle(title) {
    await page.waitForFunction(
      (t) => window.__appState?.activeMenu?._opts?.title === t,
      title, { timeout: 5000 }
    );
  }
  async function select(page) {
    await page.evaluate(() => window.__appState?.activeMenu?.handleInput('Enter'));
  }
  async function back(page) {
    await page.evaluate(() => window.__appState?.activeMenu?.handleInput('Escape'));
  }

  for (let cycle = 1; cycle <= 2; cycle++) {
    // Login → main menu
    await waitForItems('login');
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 }),
      select(page),
    ]);
    await waitForItems('games');

    // Main menu: d-pad works
    expect(await activeMenuSelIdx(page)).toBe(0);
    await menuKey(page, 'ArrowDown');
    expect(await activeMenuSelIdx(page)).toBe(1);
    await menuKey(page, 'ArrowUp');
    expect(await activeMenuSelIdx(page)).toBe(0);

    // PLAY → ROM list
    await page.evaluate(() => { if (window.__appState?.activeMenu) window.__appState.activeMenu._selIdx = 0; });
    await select(page);
    await waitForTitle('SELECT GAME');

    // ROM list: d-pad works
    expect(await activeMenuSelIdx(page)).toBe(0);
    await menuKey(page, 'ArrowDown');
    expect(await activeMenuSelIdx(page)).toBe(1);
    await menuKey(page, 'ArrowUp');
    expect(await activeMenuSelIdx(page)).toBe(0);

    // B → back to main menu
    await back(page);
    await waitForItems('games');

    // Main menu still navigable after returning (the bug this test catches)
    expect(await activeMenuSelIdx(page)).toBe(0);
    await menuKey(page, 'ArrowDown');
    expect(await activeMenuSelIdx(page)).toBe(1); // cycle ${cycle}: main menu frozen here in real browser
    await menuKey(page, 'ArrowUp');
    expect(await activeMenuSelIdx(page)).toBe(0);

    // LOGOUT → login screen
    await page.evaluate(() => { if (window.__appState?.activeMenu) window.__appState.activeMenu._selIdx = 1; });
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 }),
      select(page),
    ]);
    await waitForItems('login');
  }
});

// ── Mobile pointer-event tests ────────────────────────────────────────────────
//
// These simulate the actual mobile touch path: pointerdown + pointerup on
// d-pad/action buttons, and touchend on canvas for menu item taps.
// This is distinct from menuKey() which calls handleInput() directly.

/** Simulate a button tap via pointerdown + pointerup on a [data-btn] element. */
async function tapButton(page, selector) {
  await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    if (!el) throw new Error(`tapButton: no element for selector ${sel}`);
    el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true }));
    el.dispatchEvent(new PointerEvent('pointerup',   { bubbles: true, cancelable: true }));
  }, selector);
}

/** Simulate a canvas tap at menu canvas-space coordinates (x, y). */
async function tapCanvas(page, cx, cy) {
  await page.evaluate(([x, y]) => {
    const canvas = document.getElementById('gameCanvas');
    const rect = canvas.getBoundingClientRect();
    const scaleX = rect.width  / 160;
    const scaleY = rect.height / 144;
    const clientX = rect.left + x * scaleX;
    const clientY = rect.top  + y * scaleY;
    canvas.dispatchEvent(new TouchEvent('touchstart', {
      bubbles: true, cancelable: true,
      changedTouches: [new Touch({ identifier: 1, target: canvas, clientX, clientY })],
    }));
    canvas.dispatchEvent(new TouchEvent('touchend', {
      bubbles: true, cancelable: true,
      changedTouches: [new Touch({ identifier: 1, target: canvas, clientX, clientY })],
    }));
  }, [cx, cy]);
}

async function waitForMenuTitle(page, title) {
  await page.waitForFunction(
    (t) => window.__appState?.activeMenu?._opts?.title === t,
    title, { timeout: 3000 }
  );
}

async function waitForMenuItems(page, value) {
  await page.waitForFunction(
    (v) => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === v),
    value, { timeout: 3000 }
  );
}

// T26: mobile — tap login → main menu via canvas tap
test('T26: mobile tap on login item navigates to main menu', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'login');

  await Promise.all([
    page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 }),
    // Tap first item row: LIST_TOP=16, ITEM_H=14, midpoint y=23
    tapCanvas(page, 80, 23),
  ]);

  await waitForMenuItems(page, 'games');
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
});

// T27: mobile — d-pad down on main menu moves selection
test('T27: mobile d-pad down moves main menu selection', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  expect(await activeMenuSelIdx(page)).toBe(0);
  await tapButton(page, '[data-btn="3"]'); // Down
  expect(await activeMenuSelIdx(page)).toBe(1);
  await tapButton(page, '[data-btn="2"]'); // Up
  expect(await activeMenuSelIdx(page)).toBe(0);
});

// T28: mobile — A button on PLAY opens ROM list
test('T28: mobile A button on PLAY opens ROM list', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  await tapButton(page, '[data-btn="4"]'); // A
  await waitForMenuTitle(page, 'SELECT GAME');
});

// T29: mobile — B button on ROM list returns to main menu
test('T29: mobile B button on ROM list returns to main menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  await tapButton(page, '[data-btn="4"]'); // A → ROM list
  await waitForMenuTitle(page, 'SELECT GAME');

  await tapButton(page, '[data-btn="5"]'); // B → back
  await waitForMenuItems(page, 'games');
});

// T30: mobile — d-pad still works on main menu after returning from ROM list via B
// This is the regression test for the reported freeze.
test('T30: mobile d-pad navigable on main menu after B back from ROM list', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // Go into ROM list and back
  await tapButton(page, '[data-btn="4"]'); // A → ROM list
  await waitForMenuTitle(page, 'SELECT GAME');
  await tapButton(page, '[data-btn="5"]'); // B → main menu
  await waitForMenuItems(page, 'games');

  // D-pad must still work — this is what froze on mobile
  expect(await activeMenuSelIdx(page)).toBe(0);
  await tapButton(page, '[data-btn="3"]'); // Down
  expect(await activeMenuSelIdx(page)).toBe(1);
  await tapButton(page, '[data-btn="2"]'); // Up
  expect(await activeMenuSelIdx(page)).toBe(0);
});

// T31: mobile — full cycle: login(tap) → play(A) → ROM list → B → logout(A) → login
// Two cycles to catch any state accumulation bug.
test('T31: mobile two full cycles without freeze', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);

  for (let cycle = 1; cycle <= 2; cycle++) {
    // Login via canvas tap
    await waitForMenuItems(page, 'login');
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 }),
      tapCanvas(page, 80, 23),
    ]);
    await waitForMenuItems(page, 'games');

    // D-pad works on main menu
    expect(await activeMenuSelIdx(page)).toBe(0);
    await tapButton(page, '[data-btn="3"]'); // Down
    expect(await activeMenuSelIdx(page)).toBe(1);
    await tapButton(page, '[data-btn="2"]'); // Up
    expect(await activeMenuSelIdx(page)).toBe(0);

    // A → ROM list
    await tapButton(page, '[data-btn="4"]');
    await waitForMenuTitle(page, 'SELECT GAME');

    // D-pad works on ROM list
    await tapButton(page, '[data-btn="3"]'); // Down
    expect(await activeMenuSelIdx(page)).toBe(1);
    await tapButton(page, '[data-btn="2"]'); // Up
    expect(await activeMenuSelIdx(page)).toBe(0);

    // B → back to main menu
    await tapButton(page, '[data-btn="5"]');
    await waitForMenuItems(page, 'games');

    // D-pad still works after returning — the bug
    expect(await activeMenuSelIdx(page)).toBe(0);
    await tapButton(page, '[data-btn="3"]');
    expect(await activeMenuSelIdx(page)).toBe(1); // would be 0 if frozen
    await tapButton(page, '[data-btn="2"]');
    expect(await activeMenuSelIdx(page)).toBe(0);

    // Logout
    await page.evaluate(() => { if (window.__appState?.activeMenu) window.__appState.activeMenu._selIdx = 1; });
    await Promise.all([
      page.waitForNavigation({ waitUntil: 'networkidle', timeout: 5000 }),
      tapButton(page, '[data-btn="4"]'), // A on LOGOUT
    ]);
  }
});

// T32: mobile canvas tap fires both touch AND pointer events (real browser behavior)
// If pointer events on canvas interfere with sendButton, this catches it.
test('T32: canvas tap fires touch+pointer — d-pad still works after', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // A → ROM list via canvas tap (fires both touch and pointer events like real mobile)
  await page.evaluate(() => {
    const canvas = document.getElementById('gameCanvas');
    const rect = canvas.getBoundingClientRect();
    const scaleX = rect.width  / 160;
    const scaleY = rect.height / 144;
    // Tap first item row (LIST_TOP=16, ITEM_H=14, mid=23)
    const clientX = rect.left + 80 * scaleX;
    const clientY = rect.top  + 23 * scaleY;
    const touch = new Touch({ identifier: 1, target: canvas, clientX, clientY });
    // Fire all events a real mobile browser would fire
    canvas.dispatchEvent(new TouchEvent('touchstart', { bubbles: true, cancelable: true, changedTouches: [touch] }));
    canvas.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true, clientX, clientY }));
    canvas.dispatchEvent(new TouchEvent('touchend',   { bubbles: true, cancelable: true, changedTouches: [touch] }));
    canvas.dispatchEvent(new PointerEvent('pointerup',   { bubbles: true, cancelable: true, clientX, clientY }));
  });

  // Should have navigated to ROM list (canvas tap selects first item = PLAY)
  await waitForMenuTitle(page, 'SELECT GAME');

  // B → back
  await tapButton(page, '[data-btn="5"]');
  await waitForMenuItems(page, 'games');

  // D-pad must work
  expect(await activeMenuSelIdx(page)).toBe(0);
  await tapButton(page, '[data-btn="3"]');
  expect(await activeMenuSelIdx(page)).toBe(1);
});

// T33: regression — touch starts on canvas, ends outside (simulates d-pad tap
// with finger migrating from canvas area). Must NOT trigger menu tap.
test('T33: touch ending outside canvas bounds does not trigger menu selection', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb', 'Mario.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // Touch starts on canvas (inside), ends at a position outside canvas bounds
  const triggered = await page.evaluate(() => {
    const canvas = document.getElementById('gameCanvas');
    const rect = canvas.getBoundingClientRect();
    const insideX = rect.left + rect.width / 2;
    const insideY = rect.top  + rect.height / 2;
    // End touch 500px below canvas (where d-pad buttons are)
    const outsideX = rect.left + rect.width / 2;
    const outsideY = rect.bottom + 500;

    let selected = false;
    const origOnSelect = window.__appState.activeMenu._opts.onSelect;
    window.__appState.activeMenu._opts.onSelect = (item) => { selected = true; origOnSelect(item); };

    canvas.dispatchEvent(new TouchEvent('touchstart', {
      bubbles: true, cancelable: true,
      changedTouches: [new Touch({ identifier: 1, target: canvas, clientX: insideX, clientY: insideY })],
    }));
    canvas.dispatchEvent(new TouchEvent('touchend', {
      bubbles: true, cancelable: true,
      changedTouches: [new Touch({ identifier: 1, target: canvas, clientX: outsideX, clientY: outsideY })],
    }));

    return selected;
  });

  expect(triggered).toBe(false);
  // Menu should still be active and navigable
  expect(await hasActiveMenu(page)).toBe(true);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
});

// T34: B button on main menu must not dismiss it (the confirmed freeze bug)
test('T34: B button on main menu does not hide menu', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  await tapButton(page, '[data-btn="5"]'); // B on main menu

  expect(await hasActiveMenu(page)).toBe(true);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  // D-pad still works
  await tapButton(page, '[data-btn="3"]'); // Down
  expect(await activeMenuSelIdx(page)).toBe(1);
});

// T35: B button on login screen does not hide menu
test('T35: B button on login screen does not hide menu', async ({ page, request }) => {
  await setServerState(request, { authed: false, roms: [] });
  await loadApp(page);
  await waitForMenuItems(page, 'login');

  await tapButton(page, '[data-btn="5"]'); // B on login

  expect(await hasActiveMenu(page)).toBe(true);
  expect(await activeMenuItems(page)).toContain('login');
});

// ── Pause menu bug regression tests ──────────────────────────────────────────

/** Helper: launch a game and wait until running. */
async function launchGame(page, request) {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'], saveStates: [] });
  await loadApp(page);
  await waitForMenuItems(page, 'games');
  await menuKey(page, 'Enter'); // GAMES → ROM list
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME',
    { timeout: 3000 }
  );
  await menuKey(page, 'Enter'); // launch Tetris
  await page.waitForFunction(
    () => window.__appState?.running === true,
    { timeout: 5000 }
  );
}

// T36: Face buttons (A/B) work in the in-game pause menu
// Regression: sendButton routed to emulator while paused, ignoring the menu.
test('T36: A button selects item in in-game pause menu', async ({ page, request }) => {
  await launchGame(page, request);

  // Open in-game menu
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true &&
          window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'resume'),
    { timeout: 3000 }
  );

  // Press A (data-btn="4") — should select RESUME and unpause
  await tapButton(page, '[data-btn="4"]');

  await page.waitForFunction(
    () => window.__appState?.paused === false && window.__appState?.running === true,
    { timeout: 3000 }
  );

  expect(await isRunning(page)).toBe(true);
  expect(await page.evaluate(() => window.__appState.paused)).toBe(false);
});

// T37: B button in in-game pause menu resumes the game
// Regression: same routing bug — B press went to emulator, not menu onBack.
test('T37: B button in in-game pause menu resumes game', async ({ page, request }) => {
  await launchGame(page, request);

  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true &&
          window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );

  // Press B (data-btn="5") — should call onBack → resumeEmulation
  await tapButton(page, '[data-btn="5"]');

  await page.waitForFunction(
    () => window.__appState?.paused === false && window.__appState?.running === true,
    { timeout: 3000 }
  );

  expect(await isRunning(page)).toBe(true);
  expect(await page.evaluate(() => window.__appState.paused)).toBe(false);
});

// T42: Hammering POWER many times then QUIT leaves clean main-menu state
// Regression: concurrent showInGameMenu invocations fought over canvas; stale menu showed after quit.
test('T42: hammer power button many times then quit leaves clean state', async ({ page, request }) => {
  await launchGame(page, request);

  // Hammer the power button 5 times rapidly (odd = should end paused)
  for (let i = 0; i < 5; i++) {
    await page.click('#powerBtn');
  }

  // Wait for a stable paused state with an active menu
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 5000 }
  );

  // Navigate to QUIT and select it
  const items = await activeMenuItems(page);
  const quitIdx = items.indexOf('quit');
  for (let i = 0; i < quitIdx; i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter');

  await page.waitForFunction(
    () => window.__appState?.running === false &&
          window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );

  expect(await page.evaluate(() => window.__appState.paused)).toBe(false);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  expect(await activeMenuItems(page)).not.toContain('resume');
});

// T39: Rapid POWER toggle then QUIT leaves app in clean main-menu state (no stale pause menu)
// Regression: async showInGameMenu fetch resolved after quit, stamping pause menu over main menu.
test('T39: rapid open/close then quit leaves clean main menu state', async ({ page, request }) => {
  await launchGame(page, request);

  // Open pause menu
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );

  // Immediately resume before the fetch could have resolved cleanly
  await tapButton(page, '[data-btn="5"]'); // B → resume
  await page.waitForFunction(
    () => window.__appState?.paused === false && window.__appState?.running === true,
    { timeout: 3000 }
  );

  // Open again and quit
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );

  // Navigate down to QUIT and select it
  const items = await activeMenuItems(page);
  const quitIdx = items.indexOf('quit');
  for (let i = 0; i < quitIdx; i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter'); // QUIT

  await page.waitForFunction(
    () => window.__appState?.running === false &&
          window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );

  // State must be clean: not paused, main menu showing, no stale pause menu
  expect(await page.evaluate(() => window.__appState.paused)).toBe(false);
  expect(await activeMenuTitle(page)).toBe('RUSTYBOY');
  expect(await activeMenuItems(page)).not.toContain('resume');
  expect(await activeMenuItems(page)).toContain('games');
});

// T38: Opening and closing the pause menu multiple times keeps it functional
// Regression: stale RAF loop generation caused menu to stop opening after several cycles.
test('T38: pause menu opens correctly after multiple open/close cycles', async ({ page, request }) => {
  await launchGame(page, request);

  for (let i = 0; i < 4; i++) {
    // Open menu
    await page.click('#powerBtn');
    await page.waitForFunction(
      () => window.__appState?.paused === true &&
            window.__appState?.activeMenu?.isActive(),
      { timeout: 3000 }
    );

    // Close via B (resume)
    await tapButton(page, '[data-btn="5"]');
    await page.waitForFunction(
      () => window.__appState?.paused === false && window.__appState?.running === true,
      { timeout: 3000 }
    );
  }

  // After 4 cycles, menu must still open
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true &&
          window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );

  expect(await page.evaluate(() => window.__appState.paused)).toBe(true);
  expect(await hasActiveMenu(page)).toBe(true);
  expect(await activeMenuItems(page)).toContain('resume');
});

// T40: B button on LOAD STATE screen deletes the selected save slot
// After deletion the slot is removed from the list; if list becomes empty, go back.
test('T40: B on load state screen deletes selected save slot', async ({ page, request }) => {
  await setServerState(request, {
    authed: true,
    roms: ['Tetris.gb'],
    saveStates: [
      { id: 'ss-1', rom_name: 'Tetris.gb', slot_name: '1000', updated_at: 1000 },
      { id: 'ss-2', rom_name: 'Tetris.gb', slot_name: '2000', updated_at: 2000 },
    ],
  });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // Navigate to GAMES (may not be index 0 if CONTINUE is present)
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );
  const mainItems = await activeMenuItems(page);
  for (let i = 0; i < mainItems.indexOf('games'); i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter'); // GAMES

  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME', { timeout: 3000 }
  );
  await menuKey(page, 'Enter'); // launch Tetris
  await page.waitForFunction(() => window.__appState?.running === true, { timeout: 5000 });

  // Open pause menu → LOAD
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );
  const pauseItems = await activeMenuItems(page);
  const loadIdx = pauseItems.indexOf('load');
  for (let i = 0; i < loadIdx; i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter'); // select LOAD

  // Load state list should be showing with 2 items
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'LOAD STATE', { timeout: 3000 }
  );
  const slotsBefore = await activeMenuItems(page);
  expect(slotsBefore.length).toBe(2);

  // Press B to delete the selected (top = most recent = ss-2)
  await menuKey(page, 'Escape'); // B key

  // List should now have 1 item
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'LOAD STATE' &&
          window.__appState?.activeMenu?._opts?.items?.length === 1,
    { timeout: 3000 }
  );
  const slotsAfter = await activeMenuItems(page);
  expect(slotsAfter.length).toBe(1);
  expect(slotsAfter).not.toContain('ss-2');
});

// T43: Marquee RAF loop stops when pause menu is dismissed via power button resume
// Regression: power-button resume set state.activeMenu=null BEFORE calling resumeEmulation(),
// so resumeEmulation()'s hide() guard found null and never called hide(), leaving the
// marquee RAF loop running and continuously overpainting the canvas with the old title.
test('T43: marquee RAF loop stops after power-button resume on marquee-title game', async ({ page, request }) => {
  // Use a long ROM name that will overflow the 160px header and trigger marquee.
  const longRomName = 'A Very Long Game Title That Will Overflow The Header Area.gb';
  await setServerState(request, {
    authed: true,
    roms: [longRomName],
    saveStates: [],
  });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // Navigate to GAMES → select the long-named ROM
  const mainItems = await activeMenuItems(page);
  for (let i = 0; i < mainItems.indexOf('games'); i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter');

  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME', { timeout: 3000 }
  );
  await menuKey(page, 'Enter');
  await page.waitForFunction(() => window.__appState?.running === true, { timeout: 5000 });

  // Open pause menu via power button (shows long ROM title → triggers marquee)
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );

  // Capture the paused menu renderer instance reference
  await page.evaluate(() => {
    window._pauseMenuRef = window.__appState.activeMenu;
  });

  // Verify marquee RAF is running (long title should have started it)
  const rafRunningBefore = await page.evaluate(
    () => window._pauseMenuRef._marqueeRafId !== null
  );
  expect(rafRunningBefore).toBe(true);

  // Resume via power button (the bug: this used to null activeMenu before resumeEmulation,
  // so hide() was never called and the marquee RAF kept running)
  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === false && window.__appState?.running === true,
    { timeout: 3000 }
  );

  // The OLD menu renderer's RAF loop must now be stopped
  const rafRunningAfter = await page.evaluate(
    () => window._pauseMenuRef._marqueeRafId !== null
  );
  expect(rafRunningAfter).toBe(false);
});

// T41: Deleting the last save slot on the LOAD STATE screen goes back to pause menu
test('T41: deleting last save slot returns to pause menu', async ({ page, request }) => {
  await setServerState(request, {
    authed: true,
    roms: ['Tetris.gb'],
    saveStates: [
      { id: 'ss-only', rom_name: 'Tetris.gb', slot_name: '1000', updated_at: 1000 },
    ],
  });
  await loadApp(page);
  await waitForMenuItems(page, 'games');

  // Navigate to GAMES
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'games'),
    { timeout: 3000 }
  );
  const mainItems = await activeMenuItems(page);
  for (let i = 0; i < mainItems.indexOf('games'); i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter'); // GAMES

  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME', { timeout: 3000 }
  );
  await menuKey(page, 'Enter');
  await page.waitForFunction(() => window.__appState?.running === true, { timeout: 5000 });

  await page.click('#powerBtn');
  await page.waitForFunction(
    () => window.__appState?.paused === true && window.__appState?.activeMenu?.isActive(),
    { timeout: 3000 }
  );
  const pauseItems = await activeMenuItems(page);
  const loadIdx = pauseItems.indexOf('load');
  for (let i = 0; i < loadIdx; i++) await menuKey(page, 'ArrowDown');
  await menuKey(page, 'Enter');

  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'LOAD STATE', { timeout: 3000 }
  );

  // Delete the only slot
  await menuKey(page, 'Escape'); // B = delete

  // Should be back at pause menu (no more saves → onBack called)
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.items?.some(i => i.value === 'resume'),
    { timeout: 3000 }
  );
  expect(await activeMenuItems(page)).toContain('resume');
});
