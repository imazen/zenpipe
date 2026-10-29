// Narrow regression for the retained mock-profile preview failure. This runs
// before the unchanged full demo suite; it does not qualify native WASM codecs.
const { test, expect } = require('@playwright/test');
const path = require('path');

test('mock worker encode preview returns a decodable image and populates stats', async ({ page }) => {
  await page.route('**/pkg/zenpipe_demo.js', route => route.abort());
  await page.goto('/');
  const chooser = page.waitForEvent('filechooser');
  await page.locator('#open-btn').click();
  await (await chooser).setFiles(path.join(__dirname, 'test-image.png'));
  await expect(page.locator('#status')).toContainText('200', { timeout: 10000 });
  await expect(page.locator('#status')).toContainText('[mock]');
  const result = await page.evaluate(async () => {
    const { sendToWorker } = await import('/js/worker-client.js');
    const encoded = await sendToWorker('encode_preview', {
      format: 'jpeg', options: { quality: 85 }, adjustments: {},
    });
    const bitmap = await createImageBitmap(new Blob([encoded.data], { type: encoded.mime }));
    const actual = {
      backend: encoded.backend, mime: encoded.mime, size: encoded.size,
      bytes: encoded.data.length, prefix: Array.from(encoded.data.slice(0, 2)),
      reported: [encoded.width, encoded.height], decoded: [bitmap.width, bitmap.height],
    };
    bitmap.close();
    return actual;
  });
  expect(result.backend).toBe('mock');
  expect(result.mime).toBe('image/jpeg');
  expect(result.prefix).toEqual([255, 216]);
  expect(result.size).toBeGreaterThan(0);
  expect(result.size).toBe(result.bytes);
  expect(result.reported).toEqual([200, 150]);
  expect(result.decoded).toEqual(result.reported);
  await page.locator('#export-btn').click();
  await expect(page.locator('#export-preview-img')).toBeVisible({ timeout: 10000 });
  await expect(page.locator('#export-est-bpp')).not.toContainText('--');
});
