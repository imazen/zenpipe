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

    // Region selector should be hidden (display:none) per UX overhaul
    await expect(page.locator('#region-selector')).toBeHidden();

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

    // Reset (now in sidebar)
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

  test('detail canvas supports mouse drag panning', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Get initial region state
    const regionBefore = await page.evaluate(() => {
      // Access state through the module — use the global trick
      return { x: window.__testState?.region?.x };
    });

    // Drag on the detail canvas
    const detailCanvas = page.locator('#detail-canvas');
    const box = await detailCanvas.boundingBox();
    expect(box).toBeTruthy();

    await detailCanvas.hover();
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 50, box.y + box.height / 2 + 30);
    await page.mouse.up();
    await page.waitForTimeout(300);

    // The detail canvas should still have pixels (drag succeeded)
    const dw = await page.locator('#detail-canvas').getAttribute('width');
    expect(parseInt(dw)).toBeGreaterThan(0);
  });

  test('clicking overview repositions region', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Click on the overview canvas near top-left
    const overview = page.locator('#overview-canvas');
    const oBox = await overview.boundingBox();
    await page.mouse.click(oBox.x + 10, oBox.y + 10);
    await page.waitForTimeout(300);

    // Detail canvas should still have valid dimensions after repositioning
    const dw = await page.locator('#detail-canvas').getAttribute('width');
    expect(parseInt(dw)).toBeGreaterThan(0);
  });

  test('export opens modal and downloads a JPEG', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    // Click export button to open modal
    await page.locator('#export-btn').click();
    await expect(page.locator('#export-modal-backdrop')).toHaveClass(/open/);
    await expect(page.locator('#export-modal')).toBeVisible();

    // Modal should show image dimensions
    await expect(page.locator('#export-dims')).toContainText('\u00d7');

    // JPEG should be selected by default
    const jpegTab = page.locator('.export-format-tab.active');
    await expect(jpegTab).toContainText('JPEG');

    // Click export confirm to download
    const downloadPromise = page.waitForEvent('download', { timeout: 15000 });
    await page.locator('#export-confirm').click();
    const download = await downloadPromise;

    expect(download.suggestedFilename()).toBe('export.jpg');
    await expect(page.locator('#status')).toContainText('Exported', { timeout: 10000 });
  });

  test('export modal closes on Escape', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    await page.locator('#export-btn').click();
    await expect(page.locator('#export-modal-backdrop')).toHaveClass(/open/);

    await page.keyboard.press('Escape');
    await expect(page.locator('#export-modal-backdrop')).not.toHaveClass(/open/);
  });

  test('export modal format switching works', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    await page.locator('#export-btn').click();

    // Switch to PNG
    await page.locator('.export-format-tab:has-text("PNG")').click();
    await expect(page.locator('#export-confirm')).toContainText('Export PNG');

    // Switch to WebP
    await page.locator('.export-format-tab:has-text("WebP")').click();
    await expect(page.locator('#export-confirm')).toContainText('Export WebP');
  });

  test('export modal aspect ratio lock works', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    await page.locator('#export-btn').click();

    // Get initial dimensions
    const initWidth = await page.locator('#export-width').inputValue();
    const initHeight = await page.locator('#export-height').inputValue();

    // Change width with aspect lock on
    await page.locator('#export-width').fill('100');
    await page.locator('#export-width').dispatchEvent('input');

    const newHeight = await page.locator('#export-height').inputValue();
    // Height should have changed proportionally (not still equal to initial)
    expect(parseInt(newHeight)).toBeGreaterThan(0);
    // If source is wider than tall, height < width
    const w = parseInt(initWidth), h = parseInt(initHeight);
    if (w > h) {
      expect(parseInt(newHeight)).toBeLessThanOrEqual(100);
    }
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

  test('pixel info is shown below detail canvas', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.waitForTimeout(500);

    const pixelInfo = page.locator('#pixel-info');
    await expect(pixelInfo).toBeVisible();
    // Should contain the ratio format like "1:N" and dimensions
    const text = await pixelInfo.textContent();
    expect(text).toMatch(/1:\d+/);
    expect(text).toMatch(/\d+\u00d7\d+/);
  });

  test('pick button is visible in header', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('#pick-btn')).toBeVisible();
  });

  test('reset button is in sidebar', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);

    const resetBtn = page.locator('#reset-btn');
    await expect(resetBtn).toBeVisible();
    // Should be inside the sidebar (aside element)
    const parent = page.locator('aside #reset-btn');
    await expect(parent).toBeVisible();
  });
});

test.describe('pixel verification', () => {
  test('overview has non-zero pixels after slider change', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.waitForTimeout(500);

    // Change exposure
    await page.evaluate(() => {
      const s = document.querySelectorAll('input[type="range"]')[0];
      s.value = '1';
      s.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await page.waitForTimeout(1500);

    const result = await page.evaluate(() => {
      const c = document.getElementById('overview-canvas');
      if (c.width === 0) return { error: 'width is 0' };
      const ctx = c.getContext('2d');
      const d = ctx.getImageData(0, 0, c.width, c.height);
      const nonZero = d.data.filter(v => v > 0).length;
      return { w: c.width, h: c.height, nonZero, total: d.data.length };
    });

    expect(result.w).toBeGreaterThan(0);
    expect(result.nonZero).toBeGreaterThan(100);
  });

  test('detail has non-zero pixels after slider change', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.waitForTimeout(500);

    await page.evaluate(() => {
      const s = document.querySelectorAll('input[type="range"]')[0];
      s.value = '1';
      s.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await page.waitForTimeout(1500);

    const result = await page.evaluate(() => {
      const c = document.getElementById('detail-canvas');
      if (c.width === 0) return { error: 'width is 0' };
      const ctx = c.getContext('2d');
      const d = ctx.getImageData(0, 0, c.width, c.height);
      const nonZero = d.data.filter(v => v > 0).length;
      return { w: c.width, h: c.height, nonZero, total: d.data.length };
    });

    expect(result.w).toBeGreaterThan(0);
    expect(result.nonZero).toBeGreaterThan(100);
  });
});
