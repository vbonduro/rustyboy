/**
 * Stuck-button regression test.
 *
 * Hypothesis: if pointer capture is lost while a d-pad button is held (e.g.
 * because the finger slid off the element), the browser fires lostpointercapture
 * but NOT pointerup on the element.  Without a lostpointercapture handler the
 * emulator never receives set_button(idx, false), leaving the button stuck.
 *
 * The test:
 *   1. Start the emulator running (ROM launched).
 *   2. Spy on EmulatorHandle.set_button calls.
 *   3. Fire pointerdown on a d-pad button (simulates press).
 *   4. Fire lostpointercapture on that button WITHOUT a preceding pointerup
 *      (simulates the browser dropping capture mid-slide).
 *   5. Assert that set_button(idx, false) was called — i.e. the release reached
 *      the emulator.
 */

import { test, expect } from '@playwright/test';

const BASE = 'http://localhost:3737';

async function setServerState(request, state) {
  await request.post(`${BASE}/test/control`, { data: state });
}

async function loadApp(page) {
  await page.goto(`${BASE}/test/app`);
  await page.waitForFunction(
    () => typeof window.MenuRenderer === 'function' && window.__appState && window.__appState.activeMenu !== undefined,
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

/** Fire a synthetic PointerEvent of the given type on an element. */
async function firePointerEvent(page, selector, type, pointerId = 1) {
  await page.evaluate(([sel, evType, pid]) => {
    const el = document.querySelector(sel);
    if (!el) throw new Error(`Element not found: ${sel}`);
    const ev = new PointerEvent(evType, {
      bubbles: true,
      cancelable: true,
      pointerId: pid,
      pointerType: 'touch',
      isPrimary: true,
      clientX: el.getBoundingClientRect().left + 5,
      clientY: el.getBoundingClientRect().top  + 5,
    });
    el.dispatchEvent(ev);
  }, [selector, type, pointerId]);
}

// ── Tests ────────────────────────────────────────────────────────────────────

test('B2: normal tap sends exactly one press and one release to emulator', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  const selector = '[data-btn="2"]';

  // Full tap: pointerdown → pointerup → lostpointercapture (browser fires all three)
  await firePointerEvent(page, selector, 'pointerdown');
  await firePointerEvent(page, selector, 'pointerup');
  await firePointerEvent(page, selector, 'lostpointercapture');

  const calls = await page.evaluate(() => window._buttonCalls);
  const presses   = calls.filter(c => c.btn === 2 && c.pressed === true);
  const releases  = calls.filter(c => c.btn === 2 && c.pressed === false);

  expect(presses.length).toBe(1);
  expect(releases.length).toBe(1);
});

test('B1: lostpointercapture without pointerup sends button release to emulator', async ({ page, request }) => {
  await setServerState(request, { authed: true, roms: ['Tetris.gb'] });
  await loadApp(page);
  await launchRom(page);
  await installSpy(page);

  // btn-up is data-btn="2" (Up, index 2)
  const selector = '[data-btn="2"]';

  // Press: pointerdown
  await firePointerEvent(page, selector, 'pointerdown');

  // Verify press was recorded
  const afterPress = await page.evaluate(() => window._buttonCalls);
  expect(afterPress.some(c => c.btn === 2 && c.pressed === true)).toBe(true);

  // Clear the log so we can cleanly check for the release
  await page.evaluate(() => { window._buttonCalls = []; });

  // Simulate pointer capture being lost WITHOUT a pointerup
  await firePointerEvent(page, selector, 'lostpointercapture');

  // Release must have reached the emulator
  const afterLost = await page.evaluate(() => window._buttonCalls);
  expect(afterLost.some(c => c.btn === 2 && c.pressed === false)).toBe(true);
});
