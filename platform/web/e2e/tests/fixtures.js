import { test as base, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';

const BASE_URL = 'http://localhost:3737';

/**
 * Test fixtures that give every test its own isolated mock-server state.
 *
 * Each test gets a unique `rbTestId`, sent as the `x-rb-test` header on BOTH:
 *   - the browser context (so the app's own fetches — /api/me, /api/roms,
 *     /api/save-states, /auth/* — carry it), and
 *   - the APIRequestContext used by helpers like setServerState().
 *
 * server.cjs buckets all mutable state by that id, so no state is shared
 * between tests and the suite is safe to run fully in parallel.
 */
export const test = base.extend({
  rbTestId: async ({}, use) => {
    await use(randomUUID());
  },

  // Replace the built-in `request` fixture with one tagged with the test id.
  request: async ({ playwright, rbTestId }, use) => {
    const request = await playwright.request.newContext({
      baseURL: BASE_URL,
      extraHTTPHeaders: { 'x-rb-test': rbTestId },
    });
    await use(request);
    await request.dispose();
  },
});

// Tag the browser context (and therefore every page/fetch request) with the id.
test.beforeEach(async ({ context, rbTestId }) => {
  await context.route('**/*', route => {
    const headers = { ...route.request().headers(), 'x-rb-test': rbTestId };
    route.continue({ headers });
  });
  await context.addCookies([{ name: 'rbTestId', value: rbTestId, url: BASE_URL }]);
  await context.setExtraHTTPHeaders({ 'x-rb-test': rbTestId });
});

export { expect };
