/**
 * D-pad release regression tests.
 *
 * The production D-pad is a nipplejs static joystick. A direction press is
 * emitted when the pointer moves far enough from center; every end path must
 * release any held direction so the emulator never sees a stuck button.
 */

import { test, expect } from './fixtures.js';

const BASE = 'http://localhost:3737';

async function setServerState(target, state) {
  const api = target.request ?? target;
  await api.post(`${BASE}/test/control`, { data: state });
}

async function loadApp(page) {
  await page.goto(`${BASE}/test/app`);
  await page.waitForFunction(
    () => typeof window.MenuRenderer === 'function' && window.__appState?.activeMenu?.isActive?.(),
    { timeout: 5000 }
  );
}

/** Launch a ROM so the emulator is running and buttons route to set_button. */
async function launchRom(page) {
  // Enter from main menu → ROM list
  await page.evaluate(() => {
    const s = window.__appState;
    if (s && s.activeMenu && s.activeMenu.isActive()) s.activeMenu.handleInput('Enter');
  });
  await page.waitForFunction(
    () => window.__appState?.activeMenu?._opts?.title === 'SELECT GAME',
    { timeout: 3000 }
  );
  // Enter on first ROM → emulator starts
  await page.evaluate(() => {
    const s = window.__appState;
    if (s && s.activeMenu && s.activeMenu.isActive()) s.activeMenu.handleInput('Enter');
  });
  await page.waitForFunction(
    () => window.__appState?.running === true,
    { timeout: 5000 }
  );
}

/** Install a spy on the live EmulatorHandle instance's set_button method.
 *  Records all calls as {btn, pressed} pairs on window._buttonCalls. */
async function installSpy(page) {
  await page.evaluate(() => {
    window._buttonCalls = [];
    const s = window.__appState;
    if (!s.emulator) return;
    const orig = s.emulator.set_button.bind(s.emulator);
    s.emulator.set_button = (btn, pressed) => {
      window._buttonCalls.push({ btn, pressed });
      orig(btn, pressed);
    };
  });
}

async function startDpadDrag(page, direction, pointerId = 23) {
  await page.evaluate(([dir, pid]) => {
    const zone = document.getElementById('dpadZone');
    if (!zone) throw new Error('startDpadDrag: no #dpadZone');

    const rect = zone.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const distance = 58;
    const offsets = {
      up: [0, -distance],
      down: [0, distance],
      left: [-distance, 0],
      right: [distance, 0],
    };
    const [dx, dy] = offsets[dir];

    const fire = (target, type, x, y, buttons) => {
      target.dispatchEvent(new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        pointerId: pid,
        pointerType: 'touch',
        isPrimary: true,
        buttons,
        clientX: x,
        clientY: y,
      }));
    };

    fire(zone, 'pointerdown', centerX, centerY, 1);
    fire(document, 'pointermove', centerX + dx, centerY + dy, 1);
  }, [direction, pointerId]);
}

async function endDpadDrag(page, endType = 'pointerup', pointerId = 23) {
  await page.evaluate(([end, pid]) => {
    const zone = document.getElementById('dpadZone');
    if (!zone) throw new Error('endDpadDrag: no #dpadZone');

    const rect = zone.getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;

    document.dispatchEvent(new PointerEvent(end, {
      bubbles: true,
      cancelable: true,
      pointerId: pid,
      pointerType: 'touch',
      isPrimary: true,
      buttons: 0,
      clientX: centerX,
      clientY: centerY,
    }));
  }, [endType, pointerId]);
}

async function dpadDrag(page, direction, endType = 'pointerup') {
  await startDpadDrag(page, direction);
  await endDpadDrag(page, endType);
}

async function dpadFrontTransform(page) {
  return page.evaluate(() => {
    const front = document.querySelector('#dpadZone .front');
    return front ? getComputedStyle(front).transform : '';
  });
}

// ── Tests ────────────────────────────────────────────────────────────────────

test('B2: normal joystick drag sends exactly one press and one release to emulator', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await dpadDrag(page, 'up');

  const calls = await page.evaluate(() => window._buttonCalls);
  const presses   = calls.filter(c => c.btn === 2 && c.pressed === true);
  const releases  = calls.filter(c => c.btn === 2 && c.pressed === false);

  expect(presses.length).toBe(1);
  expect(releases.length).toBe(1);
});

test('B1: pointercancel releases held joystick direction', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await dpadDrag(page, 'up', 'pointercancel');

  const calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 2 && c.pressed === true)).toBe(true);
  expect(calls.some(c => c.btn === 2 && c.pressed === false)).toBe(true);
});

test('B1: lostpointercapture without pointerup releases held joystick direction', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await dpadDrag(page, 'up', 'lostpointercapture');

  const calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 2 && c.pressed === true)).toBe(true);
  expect(calls.some(c => c.btn === 2 && c.pressed === false)).toBe(true);
});

test('B1: lostpointercapture recenters visual joystick and accepts next drag', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await startDpadDrag(page, 'up', 23);
  await endDpadDrag(page, 'lostpointercapture', 23);
  await expect.poll(() => dpadFrontTransform(page)).toBe('matrix(1, 0, 0, 1, 0, 0)');

  await page.evaluate(() => { window._buttonCalls = []; });
  await startDpadDrag(page, 'right', 24);
  await endDpadDrag(page, 'pointerup', 24);

  const calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 0 && c.pressed === true)).toBe(true);
  expect(calls.some(c => c.btn === 0 && c.pressed === false)).toBe(true);
});

test('B1: window blur releases held joystick direction', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await startDpadDrag(page, 'up');
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));

  const calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 2 && c.pressed === true)).toBe(true);
  expect(calls.some(c => c.btn === 2 && c.pressed === false)).toBe(true);
});

test('B1: ending another pointer does not release held joystick direction', async ({ page, request }) => {
  await setServerState(page, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  await startDpadDrag(page, 'right', 23);
  await endDpadDrag(page, 'pointerup', 99);

  let calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 0 && c.pressed === true)).toBe(true);
  expect(calls.some(c => c.btn === 0 && c.pressed === false)).toBe(false);

  await endDpadDrag(page, 'pointerup', 23);

  calls = await page.evaluate(() => window._buttonCalls);
  expect(calls.some(c => c.btn === 0 && c.pressed === false)).toBe(true);
});
