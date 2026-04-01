// @ts-check
const { test, expect } = require('@playwright/test');
const path = require('path');

const TEST_IMAGE = path.join(__dirname, 'test-image.png');

/** Load the test image into the editor via file chooser. */
async function loadTestImage(page) {
  const fileChooser = page.waitForEvent('filechooser');
  await page.locator('#open-btn').click();
  const chooser = await fileChooser;
  await chooser.setFiles(TEST_IMAGE);
  // Wait for editor to initialize
  await expect(page.locator('#status')).toContainText('200', { timeout: 10000 });
}

/** Set a range slider value programmatically. */
async function setSlider(slider, value) {
  await slider.evaluate((el, v) => {
    el.value = v;
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }, String(value));
}

test.describe('zenpipe editor', () => {

  test('page loads and shows dropzone', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('#dropzone')).toBeVisible();
    await expect(page.locator('#dropzone')).toContainText('Drop an image');
  });

  test('status shows filter count after schema load', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('#status')).toContainText('filters loaded', { timeout: 5000 });
  });

  test('loading image shows editor UI with sliders', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Editor UI should appear
    await expect(page.locator('#dropzone')).toBeHidden();
    await expect(page.locator('#overview-canvas')).toBeVisible();
    await expect(page.locator('#detail-canvas')).toBeVisible();
    await expect(page.locator('#region-selector')).toBeVisible();

    // Sliders should be visible
    const groups = page.locator('.slider-group');
    const groupCount = await groups.count();
    expect(groupCount).toBeGreaterThan(0);

    const sliders = page.locator('.slider-row');
    const sliderCount = await sliders.count();
    expect(sliderCount).toBeGreaterThan(5);

    // Check a known slider exists
    await expect(page.locator('label:has-text("Stops")')).toBeVisible();
  });

  test('overview canvas has pixels after load', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const ow = await page.locator('#overview-canvas').getAttribute('width');
    const oh = await page.locator('#overview-canvas').getAttribute('height');
    expect(parseInt(ow)).toBeGreaterThan(0);
    expect(parseInt(oh)).toBeGreaterThan(0);
  });

  test('detail canvas has pixels after load', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const dw = await page.locator('#detail-canvas').getAttribute('width');
    const dh = await page.locator('#detail-canvas').getAttribute('height');
    expect(parseInt(dw)).toBeGreaterThan(0);
    expect(parseInt(dh)).toBeGreaterThan(0);
  });

  test('slider change triggers re-render', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const slider = page.locator('input[type="range"]').first();
    await expect(slider).toBeVisible();

    // Change slider
    await setSlider(slider, 0.5);
    await page.waitForTimeout(500);

    // Canvas should still have pixels
    const ow = await page.locator('#overview-canvas').getAttribute('width');
    expect(parseInt(ow)).toBeGreaterThan(0);
  });

  test('reset button resets all sliders', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Change a slider
    const slider = page.locator('input[type="range"]').first();
    await setSlider(slider, 0.8);
    await page.waitForTimeout(200);

    // Reset
    await page.locator('#reset-btn').click();
    await page.waitForTimeout(300);

    // All sliders should be at (or very near) their identity
    const sliders = await page.locator('input[type="range"]').all();
    for (const s of sliders) {
      const val = parseFloat(await s.inputValue());
      const identity = parseFloat(await s.getAttribute('data-identity'));
      expect(val).toBeCloseTo(identity, 1);
    }
  });

  test('region selector is visible and draggable', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const selector = page.locator('#region-selector');
    await expect(selector).toBeVisible();

    const box1 = await selector.boundingBox();
    expect(box1).toBeTruthy();
    expect(box1.width).toBeGreaterThan(10);
    expect(box1.height).toBeGreaterThan(10);

    // Drag it
    await selector.hover();
    await page.mouse.down();
    await page.mouse.move(box1.x + box1.width / 2 + 30, box1.y + box1.height / 2 + 20);
    await page.mouse.up();
    await page.waitForTimeout(300);

    const box2 = await selector.boundingBox();
    // Should have moved (allow 1px tolerance)
    expect(Math.abs(box2.x - box1.x) + Math.abs(box2.y - box1.y)).toBeGreaterThan(1);
  });

  test('clicking overview repositions region', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const selector = page.locator('#region-selector');
    const box1 = await selector.boundingBox();

    // Click on the overview canvas near top-left
    const overview = page.locator('#overview-canvas');
    const oBox = await overview.boundingBox();
    await page.mouse.click(oBox.x + 10, oBox.y + 10);
    await page.waitForTimeout(300);

    const box2 = await selector.boundingBox();
    expect(Math.abs(box2.x - box1.x) + Math.abs(box2.y - box1.y)).toBeGreaterThan(1);
  });

  test('export downloads a JPEG', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const downloadPromise = page.waitForEvent('download', { timeout: 15000 });
    await page.locator('#export-btn').click();
    const download = await downloadPromise;

    expect(download.suggestedFilename()).toBe('export.jpg');
    await expect(page.locator('#status')).toContainText('Exported', { timeout: 10000 });
  });

  test('double-click slider resets to identity', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const slider = page.locator('input[type="range"]').first();
    const identity = await slider.getAttribute('data-identity');

    // Change it
    await setSlider(slider, 0.7);
    await page.waitForTimeout(100);

    // Double-click to reset
    await slider.dblclick();
    await page.waitForTimeout(100);

    const val = parseFloat(await slider.inputValue());
    expect(val).toBeCloseTo(parseFloat(identity), 4);
  });

  test('status shows backend type', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const status = await page.locator('#status').textContent();
    // Should show either [mock] or [WASM]
    expect(status).toMatch(/\[(mock|WASM)\]/);
  });

  test('multiple slider changes produce valid output', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Change several sliders
    const sliders = await page.locator('input[type="range"]').all();
    for (let i = 0; i < Math.min(3, sliders.length); i++) {
      await setSlider(sliders[i], 0.3);
    }
    await page.waitForTimeout(500);

    // Both canvases should still have valid dimensions
    const ow = await page.locator('#overview-canvas').getAttribute('width');
    const dw = await page.locator('#detail-canvas').getAttribute('width');
    expect(parseInt(ow)).toBeGreaterThan(0);
    expect(parseInt(dw)).toBeGreaterThan(0);
  });
});
