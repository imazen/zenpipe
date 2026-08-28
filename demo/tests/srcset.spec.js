// @ts-check
// Srcset generator + export worker pool (zenpipe#22). Runs on the mock
// (OffscreenCanvas) backend when pkg/ is absent, so the assertions are
// about batching, packaging, progress and cancel — not codec bytes.
const { test, expect } = require('@playwright/test');
const path = require('path');
const fs = require('fs');
const zlib = require('zlib');

const TEST_IMAGE = path.join(__dirname, 'test-image.png');

/** Load the test image into the editor via file chooser. */
async function loadTestImage(page) {
  const fileChooser = page.waitForEvent('filechooser');
  await page.locator('#open-btn').click();
  const chooser = await fileChooser;
  await chooser.setFiles(TEST_IMAGE);
  await expect(page.locator('#status')).toContainText('200', { timeout: 10000 });
}

/** Minimal zip reader: central directory → [{ name, size, crcOk }]. */
function readZip(buf) {
  const eocd = buf.length - 22;
  if (buf.readUInt32LE(eocd) !== 0x06054b50) throw new Error('no EOCD');
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const out = [];
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(p) !== 0x02014b50) throw new Error('bad central header');
    const crc = buf.readUInt32LE(p + 16);
    const size = buf.readUInt32LE(p + 20);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    const local = buf.readUInt32LE(p + 42);
    const name = buf.subarray(p + 46, p + 46 + nameLen).toString('utf8');
    if (buf.readUInt32LE(local) !== 0x04034b50) throw new Error('bad local header');
    const lname = buf.readUInt16LE(local + 26);
    const lextra = buf.readUInt16LE(local + 28);
    const data = buf.subarray(local + 30 + lname + lextra, local + 30 + lname + lextra + size);
    out.push({ name, size, crcOk: zlib.crc32(data) === crc });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

test.describe('srcset generator', () => {
  test('generates a zip of width×format encodes and a picture snippet', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    await expect(page.locator('#srcset-section')).toBeVisible();

    // 9999 is capped to the 200 px source; JPEG + WebP are checked by default.
    await page.fill('#srcset-widths', '120, 160, 9999');
    await page.fill('#srcset-basename', 'pic');
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: 30000 }),
      page.locator('#srcset-generate').click(),
    ]);
    expect(download.suggestedFilename()).toBe('pic-srcset.zip');
    const entries = readZip(fs.readFileSync(await download.path()));
    expect(entries.map(e => e.name).sort()).toEqual([
      'pic-120w.jpg', 'pic-120w.webp',
      'pic-160w.jpg', 'pic-160w.webp',
      'pic-200w.jpg', 'pic-200w.webp',
    ]);
    for (const e of entries) {
      expect(e.size, e.name).toBeGreaterThan(0);
      expect(e.crcOk, e.name).toBe(true);
    }

    const snippet = await page.locator('#srcset-snippet').inputValue();
    expect(snippet).toContain('<picture>');
    expect(snippet).toContain('<source type="image/webp" srcset="pic-120w.webp 120w, pic-160w.webp 160w, pic-200w.webp 200w"');
    expect(snippet).toContain('<img src="pic-200w.jpg" srcset="pic-120w.jpg 120w, pic-160w.jpg 160w, pic-200w.jpg 200w"');
    await expect(page.locator('#srcset-progress-text')).toContainText('Done: 6 files');

    // The batch ran on the export pool, spread over more than one worker,
    // and nothing is left running.
    const stats = await page.evaluate(() => window.__zenpipeExportPool.stats());
    expect(stats.completed).toBeGreaterThanOrEqual(6);
    expect(stats.spawned).toBeGreaterThanOrEqual(2);
    expect(stats.running).toBe(0);
    expect(stats.failed).toBe(0);
  });

  test('cancel terminates the pool workers and produces no download', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    // Enough jobs to still be running when Cancel lands on either backend
    // (the mock encodes ~200 jobs/s): 180 distinct widths ≤ the 200 px
    // source × 4 formats = 720 jobs. JXL stays unchecked: on the deployed
    // WASM build every JXL export ends in `unreachable` (measured
    // 2026-08-28 at 100/160/200 px; JPEG/WebP/AVIF/PNG all encode), which
    // would turn this into a codec test.
    const widths = Array.from({ length: 180 }, (_, i) => 20 + i).join(',');
    await page.fill('#srcset-widths', widths);
    for (const v of ['avif', 'png']) {
      await page.locator(`#srcset-formats input[value="${v}"]`).check();
    }
    let downloaded = false;
    page.on('download', () => { downloaded = true; });
    await page.locator('#srcset-generate').click();
    await expect(page.locator('#srcset-cancel')).toBeEnabled();
    await page.locator('#srcset-cancel').click();
    await expect(page.locator('#srcset-progress-text')).toHaveText('Cancelled', { timeout: 10000 });
    await expect(page.locator('#srcset-generate')).toBeEnabled();
    const stats = await page.evaluate(() => window.__zenpipeExportPool.stats());
    expect(stats.alive).toBe(0);
    expect(stats.running).toBe(0);
    expect(stats.completed).toBeLessThan(600);
    await page.waitForTimeout(500);
    expect(downloaded).toBe(false);

    // The pool respawns for the next batch.
    await page.fill('#srcset-widths', '100');
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: 30000 }),
      page.locator('#srcset-generate').click(),
    ]);
    expect(download.suggestedFilename()).toBe('image-srcset.zip');
  });

  test('a plain export runs on the pool, keeping the primary worker interactive', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      page.locator('#export-confirm').click(),
    ]);
    expect(download.suggestedFilename()).toMatch(/\.jpg$/);
    const stats = await page.evaluate(() => window.__zenpipeExportPool.stats());
    expect(stats.completed).toBe(1);
    expect(stats.spawned).toBe(1);
  });

  test('loading another image discards the previous image\'s pool workers', async ({ page }) => {
    await page.goto('/');
    await loadTestImage(page);
    await page.locator('#export-btn').click();
    await Promise.all([
      page.waitForEvent('download'),
      page.locator('#export-confirm').click(),
    ]);
    const before = await page.evaluate(() => window.__zenpipeExportPool.stats());
    expect(before.alive).toBe(1);
    await loadTestImage(page);
    // The status already said "200" from the first load, so wait for the
    // new image epoch rather than the status text.
    await expect
      .poll(() => page.evaluate(() => window.__zenpipeExportPool.stats().epoch))
      .toBe(before.epoch + 1);
    const stats = await page.evaluate(() => window.__zenpipeExportPool.stats());
    expect(stats.alive).toBe(0);
    // And the next export spawns a fresh worker on the new image.
    await page.locator('#export-btn').click();
    await Promise.all([
      page.waitForEvent('download'),
      page.locator('#export-confirm').click(),
    ]);
    expect((await page.evaluate(() => window.__zenpipeExportPool.stats())).spawned).toBe(2);
  });
});
