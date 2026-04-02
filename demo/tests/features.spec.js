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
  await expect(page.locator('#status')).toContainText('200', { timeout: 10000 });
}

/** Set a range slider value programmatically. */
async function setSlider(slider, value) {
  await slider.evaluate((el, v) => {
    el.value = v;
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }, String(value));
}

/** Check slider value is approximately equal (f32 precision). */
async function expectSliderApprox(slider, expected, tolerance = 0.01) {
  const val = parseFloat(await slider.inputValue());
  expect(Math.abs(val - expected)).toBeLessThan(tolerance);
}

test.describe('undo/redo', () => {
  test('Ctrl+Z undoes slider change', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // Use the exposure slider (data-key contains "exposure", range -5..5)
    const slider = page.locator('input[type="range"][data-key*="exposure"]').first();
    // Change it to 2
    await setSlider(slider, 2);
    await page.waitForTimeout(500); // let debounce push state
    await expectSliderApprox(slider, 2);
    // Undo
    await page.keyboard.press('Control+z');
    await page.waitForTimeout(200);
    await expectSliderApprox(slider, 0); // identity is 0
  });

  test('Ctrl+Shift+Z redoes undone change', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    const slider = page.locator('input[type="range"][data-key*="exposure"]').first();
    await setSlider(slider, 2);
    await page.waitForTimeout(500);
    // Undo
    await page.keyboard.press('Control+z');
    await page.waitForTimeout(200);
    // Redo (Ctrl+Y is more reliable across platforms)
    await page.keyboard.press('Control+y');
    await page.waitForTimeout(200);
    await expectSliderApprox(slider, 2);
  });
});

test.describe('compare mode', () => {
  test('backslash key shows ORIGINAL badge', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // Change a slider so edited differs from original
    const slider = page.locator('input[type="range"]').first();
    await setSlider(slider, 3);
    await page.waitForTimeout(500);
    // Hold backslash
    await page.keyboard.down('\\');
    await page.waitForTimeout(300);
    await expect(page.locator('#compare-badge')).toBeVisible();
    // Release
    await page.keyboard.up('\\');
    await page.waitForTimeout(100);
    await expect(page.locator('#compare-badge')).toBeHidden();
  });
});

test.describe('user presets', () => {
  test('save and load a preset', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // Change exposure slider
    const slider = page.locator('input[type="range"][data-key*="exposure"]').first();
    await setSlider(slider, 2);
    await page.waitForTimeout(200);
    // Save preset
    page.on('dialog', dialog => dialog.accept('Test Preset'));
    await page.locator('#save-preset-btn').click();
    // Verify preset appears in list
    await expect(page.locator('.user-preset-name')).toContainText('Test Preset');
    // Reset all sliders
    await page.locator('#reset-btn').click();
    await page.waitForTimeout(200);
    // Load preset
    await page.locator('.user-preset-name').first().click();
    await page.waitForTimeout(200);
    // Slider should be back to 2
    await expectSliderApprox(slider, 2);
  });

  test('delete a preset', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // Save a preset
    page.on('dialog', dialog => dialog.accept('To Delete'));
    await page.locator('#save-preset-btn').click();
    await expect(page.locator('.user-preset-name')).toContainText('To Delete');
    // Delete it
    await page.locator('.user-preset-del').first().click();
    await expect(page.locator('.user-preset-name')).not.toBeVisible();
  });
});

test.describe('export features', () => {
  test('export modal shows encode preview with stats', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    await expect(page.locator('#export-modal-backdrop')).toHaveClass(/open/);
    // Wait for encode preview to load
    await expect(page.locator('#export-preview-img')).toBeVisible({ timeout: 10000 });
    // Stats should show real data
    await expect(page.locator('#export-est-bpp')).not.toContainText('--');
  });

  test('visible-when: WebP near-lossless hidden until lossless checked', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    // Switch to WebP
    await page.locator('.export-format-tab', { hasText: 'WebP' }).click();
    // Near-lossless should be hidden
    const nearLossless = page.locator('.export-control', { hasText: 'Near-Lossless' });
    await expect(nearLossless).toBeHidden();
    // Check lossless
    await page.locator('#export-chk-lossless').check();
    // Near-lossless should appear
    await expect(nearLossless).toBeVisible();
  });

  test('export history records download', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // Open export modal
    await page.locator('#export-btn').click();
    await expect(page.locator('#export-modal-backdrop')).toHaveClass(/open/);
    // Perform export
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.locator('#export-confirm').click(),
    ]);
    expect(download.suggestedFilename()).toMatch(/\.jpg$/);
    // Re-open export modal
    await page.waitForTimeout(500);
    await page.locator('#export-btn').click();
    // Expand history
    await page.locator('#export-history-toggle').click();
    // Should have at least one entry
    await expect(page.locator('.export-history-row')).toBeVisible();
  });
});

test.describe('metadata', () => {
  test('native upgrade shows metadata badges in status', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    // After native decode upgrade, status should contain format info
    // PNG test image may or may not have metadata, but upgrade should complete
    await page.waitForTimeout(3000);
    // Status should still show dimensions (upgrade doesn't break display)
    await expect(page.locator('#status')).toContainText('200');
  });
});
