// @ts-check
// Browser checks for the media-suite contract shell (zenpipe#113).
// These qualify page behavior only — the backend is the labeled model,
// never a codec qualification. Run: cd demo && npx playwright test media-suite
const { test, expect } = require('@playwright/test');

/** Read one RGBA pixel from a canvas. */
async function pixel(page, selector, x, y) {
  return page.evaluate(
    ([sel, px, py]) => {
      const c = document.querySelector(sel);
      const d = c.getContext('2d').getImageData(px, py, 1, 1).data;
      return [d[0], d[1], d[2], d[3]];
    },
    [selector, x, y],
  );
}

test.describe('media lab contract shell', () => {
  test('shows the MODEL badge prominently, not a codec claim', async ({ page }) => {
    await page.goto('/lab/');
    const badge = page.locator('#backend-badge');
    await expect(badge).toBeVisible();
    await expect(badge).toHaveText('MODEL');
    await expect(badge).toHaveAttribute('title', /no codec or memory-performance qualification/i);
    // The page must also state the limitation in visible copy.
    await expect(page.locator('.notes').last()).toContainText('MODEL backend');
    await expect(page.locator('.notes').last()).toContainText('not');
  });

  test('links back to the editor', async ({ page }) => {
    await page.goto('/lab/');
    const link = page.locator('#editor-link');
    await expect(link).toBeVisible();
    await expect(link).toHaveAttribute('href', '../index.html');
  });

  test('fixture canvas holds exact deterministic pixels', async ({ page }) => {
    await page.goto('/lab/');
    // generation 0: R=(x*3+y*5)%256, G=(x*7)%256, B=(y*11)%256, A=255
    expect(await pixel(page, '#fixture-canvas', 0, 0)).toEqual([0, 0, 0, 255]);
    expect(await pixel(page, '#fixture-canvas', 1, 0)).toEqual([3, 7, 0, 255]);
    expect(await pixel(page, '#fixture-canvas', 0, 1)).toEqual([5, 0, 11, 255]);
    expect(await pixel(page, '#fixture-canvas', 95, 63)).toEqual([600 % 256, 665 % 256, 693 % 256, 255]);
    await expect(page.locator('#fixture-caption')).toHaveText('generation 0');
  });

  test('submit via keyboard runs a model task and updates the preview exactly', async ({ page }) => {
    await page.goto('/lab/');
    await page.locator('#op-select').selectOption('invert');
    // Keyboard-only path: focus the control and activate with Enter.
    await page.locator('#submit-btn').focus();
    await page.keyboard.press('Enter');

    await expect(page.locator('#preview-caption')).toContainText('op-', { timeout: 5000 });
    const row = page.locator('#task-body tr').first();
    await expect(row.locator('.st')).toContainText('completed');
    // invert(pixel) = 255 - channel, alpha kept.
    const before = await pixel(page, '#fixture-canvas', 0, 0);
    const after = await pixel(page, '#preview-canvas', 0, 0);
    expect(after).toEqual(before.map((v, i) => (i === 3 ? v : 255 - v)));
  });

  test('task cancel reaches a cancelled state, never a fake completion', async ({ page }) => {
    await page.goto('/lab/');
    await page.locator('#submit-btn').click();
    const cancelBtn = page.locator('#task-body button', { hasText: 'Cancel' }).first();
    await expect(cancelBtn).toBeVisible();
    await cancelBtn.click();
    const st = page.locator('#task-body .st').first();
    await expect(st).toContainText(/cancelling|cancelled/, { timeout: 5000 });
    await expect(st).toContainText('cancelled', { timeout: 5000 });
    await expect(page.locator('#preview-caption')).toHaveText('no output yet');
  });

  test('unsupported media kinds refuse as unqualified', async ({ page }) => {
    await page.goto('/lab/');
    await page.locator('#op-select').selectOption('mp4-clip');
    await page.locator('#submit-btn').click();
    const st = page.locator('#task-body .st').first();
    await expect(st).toContainText('failed');
    await expect(st).toContainText('unsupported');
    await expect(page.locator('#status')).toContainText('refused');
    await expect(page.locator('#preview-caption')).toHaveText('no output yet');
  });

  test('unknown estimate is shown as unknown, distinct from the reservation', async ({ page }) => {
    await page.goto('/lab/');
    await page.locator('#estimate-select').selectOption('unknown');
    await expect(page.locator('#mem-estimate')).toContainText('unknown');
    await page.locator('#submit-btn').click();
    const row = page.locator('#task-body tr').first();
    await expect(row).toContainText('unknown');
    // The declared reservation is still booked and shown numerically.
    await expect(row).toContainText('B');
    await expect(page.locator('#mem-reserved')).toContainText('B /');
  });

  test('teardown reports honestly and refuses further submits', async ({ page }) => {
    await page.goto('/lab/');
    await page.locator('#submit-btn').click();
    await page.locator('#teardown-btn').click();
    await expect(page.locator('#status')).toContainText(/teardown/, { timeout: 5000 });
    await page.locator('#submit-btn').click();
    await expect(page.locator('#status')).toContainText('torn down');
  });

  test('no horizontal overflow at 390px', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 800 });
    await page.goto('/lab/');
    await expect(page.locator('#backend-badge')).toBeVisible();
    const widths = await page.evaluate(() => ({
      doc: document.documentElement.scrollWidth,
      body: document.body.scrollWidth,
    }));
    expect(widths.doc).toBeLessThanOrEqual(390);
    expect(widths.body).toBeLessThanOrEqual(390);
  });
});
