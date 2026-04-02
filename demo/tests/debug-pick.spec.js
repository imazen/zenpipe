const { test, expect } = require('@playwright/test');
test('debug pick button', async ({ page }) => {
  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push('PAGE: ' + e.message));
  await page.goto('/');
  await expect(page.locator('#status')).toContainText('filters', { timeout: 5000 });
  
  // Click pick
  await page.locator('#pick-btn').click();
  await page.waitForTimeout(1000);
  
  // Check what happened
  const state = await page.evaluate(() => ({
    dropzoneHidden: document.getElementById('dropzone').classList.contains('hidden'),
    popoverOpen: document.getElementById('photo-picker-popover')?.classList.contains('open'),
    samplePhotos: document.getElementById('sample-photos')?.children.length,
    popoverPhotos: document.getElementById('popover-photos')?.children.length,
  }));
  console.log('State:', JSON.stringify(state));
  console.log('Errors:', JSON.stringify(errors));
});
