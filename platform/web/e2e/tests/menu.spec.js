import { test, expect } from '@playwright/test';

// Helpers ----------------------------------------------------------------

/**
 * Navigate to the menu test fixture and return a handle to the canvas locator.
 */
async function loadMenuPage(page) {
  await page.goto('/test/menu');
  await page.waitForFunction(() => typeof window.MenuRenderer === 'function');
  return page.locator('#testCanvas');
}

/**
 * Run JS in page context to create a MenuRenderer, call show() and return
 * the menu instance on window so subsequent evaluate() calls can reach it.
 */
async function showMenu(page, options) {
  await page.evaluate((opts) => {
    const canvas = document.getElementById('testCanvas');
    window._testMenu = new window.MenuRenderer(canvas);
    window._testMenu.show(opts);
  }, options);
}

// Read a single pixel from the canvas at (x, y).  Returns [r, g, b, a].
async function getPixel(page, x, y) {
  return page.evaluate(([px, py]) => {
    const canvas = document.getElementById('testCanvas');
    const ctx = canvas.getContext('2d');
    const d = ctx.getImageData(px, py, 1, 1).data;
    return [d[0], d[1], d[2], d[3]];
  }, [x, y]);
}

// Convert hex color like '#0F380F' to [r, g, b].
function hexToRgb(hex) {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return [r, g, b];
}

// Tests ------------------------------------------------------------------

test('menu renders to canvas with GB colors', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'SELECT GAME',
    items: [
      { label: 'Tetris',  value: 'tetris'  },
      { label: 'Mario',   value: 'mario'   },
      { label: 'Zelda',   value: 'zelda'   },
    ],
  });

  const canvas = page.locator('#testCanvas');

  // Canvas should not be blank — take a screenshot and verify it exists
  const screenshot = await canvas.screenshot();
  expect(screenshot.length).toBeGreaterThan(0);

  // Background color at (0, 80) should be C0 = #0F380F
  // (row 80 is in the item list area between header and footer, but row 80
  // is between rows — let's pick a pixel that is definitely background.
  // Items start at LIST_TOP=16, each is 14px tall.  Row 80 is between
  // item rows: row 0 is y 16-29, row 1 is 30-43, row 2 is 44-57, row 3 is 58-71,
  // row 4 is 72-85.  We have 3 items so row 80 would be after the 3rd item.
  // Actually let's use x=0, y=80 which is past 3 items so background C0.)
  const [r, g, b] = await getPixel(page, 0, 80);
  const [er, eg, eb] = hexToRgb('#0F380F');
  expect(r).toBe(er);
  expect(g).toBe(eg);
  expect(b).toBe(eb);
});

test('menu shows title and items - header fill color present', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'SELECT GAME',
    items: [
      { label: 'Tetris', value: 't' },
      { label: 'Mario',  value: 'm' },
    ],
  });

  // Header spans y=0 to y=13 (HEADER_H=14), fill color C1 = #306230
  // Check pixel at (2, 7) — left edge of header, no text here
  const [r, g, b] = await getPixel(page, 2, 7);
  const [er, eg, eb] = hexToRgb('#306230');
  expect(r).toBe(er);
  expect(g).toBe(eg);
  expect(b).toBe(eb);
});

test('keyboard ArrowDown moves selection', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'GAMES',
    items: [
      { label: 'A', value: 'a' },
      { label: 'B', value: 'b' },
      { label: 'C', value: 'c' },
    ],
  });

  // Initial selection is 0
  let selIdx = await page.evaluate(() => window._testMenu._selIdx);
  expect(selIdx).toBe(0);

  // Move down once -> index 1
  await page.evaluate(() => window._testMenu.handleInput('ArrowDown'));
  selIdx = await page.evaluate(() => window._testMenu._selIdx);
  expect(selIdx).toBe(1);

  // Move down again -> index 2
  await page.evaluate(() => window._testMenu.handleInput('ArrowDown'));
  selIdx = await page.evaluate(() => window._testMenu._selIdx);
  expect(selIdx).toBe(2);

  // Move down again -> wraps to index 0
  await page.evaluate(() => window._testMenu.handleInput('ArrowDown'));
  selIdx = await page.evaluate(() => window._testMenu._selIdx);
  expect(selIdx).toBe(0);
});

test('keyboard ArrowUp wraps to last item', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'GAMES',
    items: [
      { label: 'A', value: 'a' },
      { label: 'B', value: 'b' },
      { label: 'C', value: 'c' },
    ],
  });

  // ArrowUp from index 0 should wrap to index 2
  await page.evaluate(() => window._testMenu.handleInput('ArrowUp'));
  const selIdx = await page.evaluate(() => window._testMenu._selIdx);
  expect(selIdx).toBe(2);
});

test('Enter key calls onSelect and hides menu', async ({ page }) => {
  await loadMenuPage(page);

  // Set up menu with a spy via page.evaluate
  await page.evaluate(() => {
    const canvas = document.getElementById('testCanvas');
    window._testMenu = new window.MenuRenderer(canvas);
    window._selectedItem = null;
    window._testMenu.show({
      title: 'GAMES',
      items: [
        { label: 'Tetris', value: 'tetris' },
        { label: 'Mario',  value: 'mario'  },
      ],
      onSelect: (item) => { window._selectedItem = item; },
    });
  });

  await page.evaluate(() => window._testMenu.handleInput('Enter'));

  const isActive = await page.evaluate(() => window._testMenu.isActive());
  expect(isActive).toBe(false);

  const selected = await page.evaluate(() => window._selectedItem);
  expect(selected).not.toBeNull();
  expect(selected.value).toBe('tetris');
});

test('Escape key calls onBack and hides menu', async ({ page }) => {
  await loadMenuPage(page);

  await page.evaluate(() => {
    const canvas = document.getElementById('testCanvas');
    window._testMenu = new window.MenuRenderer(canvas);
    window._backCalled = false;
    window._testMenu.show({
      title: 'GAMES',
      items: [{ label: 'Tetris', value: 'tetris' }],
      onBack: () => { window._backCalled = true; },
    });
  });

  await page.evaluate(() => window._testMenu.handleInput('Escape'));

  const isActive = await page.evaluate(() => window._testMenu.isActive());
  expect(isActive).toBe(false);

  const backCalled = await page.evaluate(() => window._backCalled);
  expect(backCalled).toBe(true);
});

test('tap on item calls onSelect', async ({ page }) => {
  await loadMenuPage(page);

  await page.evaluate(() => {
    const canvas = document.getElementById('testCanvas');
    window._testMenu = new window.MenuRenderer(canvas);
    window._tappedItem = null;
    window._testMenu.show({
      title: 'GAMES',
      items: [
        { label: 'Tetris', value: 'tetris' },
        { label: 'Mario',  value: 'mario'  },
        { label: 'Zelda',  value: 'zelda'  },
      ],
      onSelect: (item) => { window._tappedItem = item; },
    });
  });

  // LIST_TOP = HEADER_H + 2 = 16.  First item row: y in [16, 29], midpoint y=23.
  // handleTap(x, y) — x=80, y=22 is inside first item row.
  await page.evaluate(() => window._testMenu.handleTap(80, 22));

  const tapped = await page.evaluate(() => window._tappedItem);
  expect(tapped).not.toBeNull();
  expect(tapped.value).toBe('tetris');
});

// Marquee tests — title scrolling behavior

test('short title has zero marquee offset', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'HI',
    items: [{ label: 'x', value: 'x' }],
  });

  // Short title fits — marquee offset should stay 0
  await page.waitForTimeout(200);
  const offset = await page.evaluate(() => window._testMenu._marqueeOffset);
  expect(offset).toBe(0);
});

test('long title starts at zero offset then scrolls after 1s pause', async ({ page }) => {
  await loadMenuPage(page);
  // Title long enough to overflow the 160px header at 8px monospace
  await showMenu(page, {
    title: 'SUPER LONG GAME TITLE THAT WILL NOT FIT',
    items: [{ label: 'x', value: 'x' }],
  });

  // Immediately after show, offset should be 0 (in the pause phase)
  const offsetAtStart = await page.evaluate(() => window._testMenu._marqueeOffset);
  expect(offsetAtStart).toBe(0);

  // After >1s the scroll should have begun (offset > 0)
  await page.waitForTimeout(1400);
  const offsetAfterPause = await page.evaluate(() => window._testMenu._marqueeOffset);
  expect(offsetAfterPause).toBeGreaterThan(0);
});

test('marquee resets to zero after title scrolls fully off screen', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'SUPER LONG GAME TITLE THAT WILL NOT FIT',
    items: [{ label: 'x', value: 'x' }],
  });

  // _marqueeScrollMax is the total px to scroll before reset (TEXT_PAD + titleWidth)
  // Fast-forward: force scroll phase with phaseAt far in the past so elapsed > scrollMax
  await page.evaluate(() => {
    const menu = window._testMenu;
    menu._marqueePhase   = 'scroll';
    menu._marqueePhaseAt = performance.now() - 100000; // 100s ago → way past any title width
  });

  // Give one RAF tick to process
  await page.waitForTimeout(100);

  const offset = await page.evaluate(() => window._testMenu._marqueeOffset);
  expect(offset).toBe(0);
});

test('marquee RAF loop stops when hide() is called', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'SUPER LONG GAME TITLE THAT WILL NOT FIT',
    items: [{ label: 'x', value: 'x' }],
  });

  await page.evaluate(() => window._testMenu.hide());

  const rafActive = await page.evaluate(() => window._testMenu._marqueeRafId !== null);
  expect(rafActive).toBe(false);
});

test('menu hides when hide() is called', async ({ page }) => {
  await loadMenuPage(page);
  await showMenu(page, {
    title: 'GAMES',
    items: [{ label: 'Tetris', value: 'tetris' }],
  });

  let isActive = await page.evaluate(() => window._testMenu.isActive());
  expect(isActive).toBe(true);

  await page.evaluate(() => window._testMenu.hide());

  isActive = await page.evaluate(() => window._testMenu.isActive());
  expect(isActive).toBe(false);
});
