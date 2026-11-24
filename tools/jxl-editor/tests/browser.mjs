import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { once } from "node:events";
import { createServer } from "node:http";
import { extname } from "node:path";
import { chromium } from "playwright";
import init, {
  decode_jxl, encode_cmyk_jxl, encode_jxl, encode_layers, generate_minimal_cmyk_icc,
} from "../dist/wasm/relaxed/jxl_editor_codecs.js";
import initNative, {
  native_extract_cmyk, native_jxl_info, native_jxl_timeline,
} from "../dist/wasm/relaxed/jxl_editor_native.js";
import { box, jxlBoxes, opaqueBoxes, readProject } from "../web/project.mjs";

const baseUrl = process.env.EDITOR_URL ?? "http://127.0.0.1:3761";
const output = process.env.EDITOR_OUTPUT ?? "test-results";
await mkdir(output, { recursive: true });
await init({ module_or_path: await readFile(new URL("../dist/wasm/relaxed/jxl_editor_codecs_bg.wasm", import.meta.url)) });
await initNative({ module_or_path: await readFile(new URL("../dist/wasm/relaxed/jxl_editor_native_bg.wasm", import.meta.url)) });

function assertBytesEqual(actual, expected, label = "buffer") {
  if (actual.length === expected.length && Buffer.compare(actual, expected) === 0) return;
  const n = Math.min(actual.length, expected.length);
  let first = -1;
  for (let i = 0; i < n; i++) {
    if (actual[i] !== expected[i]) { first = i; break; }
  }
  assert.fail(`${label} mismatch: actual ${actual.length} B vs expected ${expected.length} B (first diff at byte ${first}: ${actual[first]} vs ${expected[first]})`);
}

function rgba(width, height, pixel) {
  const result = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) result.set(pixel(x, y), 4 * (y * width + x));
  }
  return result;
}

const image = rgba(127, 93, (x, y) => [x + 47, y * 2, x + y, 255]);
const overlay = rgba(31, 21, (x, y) => [220, 140 + x, 2 + y, (x + y) % 3 ? 255 : 177]);
const alphaStrided = Uint8Array.of(5, 3, 1, 0, 17, 33, 129, 1, 41, 55, 23, 255,
  83, 83, 83, 83, 7, 3, 7, 128, 200, 23, 199, 93, 5, 55, 22, 254);
const odd = encode_jxl(alphaStrided, 3, 2, 16);
const oddExpected = Buffer.concat([alphaStrided.subarray(0, 12), alphaStrided.subarray(16)]);
assert.deepEqual(Buffer.from(decode_jxl(odd).into_pixels()), oddExpected);
assert.throws(() => encode_jxl(alphaStrided, 3, 2, 11));
assert.throws(() => encode_jxl(alphaStrided.subarray(0, 27), 3, 2, 16));
await writeFile(`${output}/odd-strided.jxl`, odd);
await writeFile(`${output}/odd-strided.rgba`, oddExpected);
const bare = encode_jxl(image, 127, 93, 127 * 4);
const second = encode_jxl(overlay, 31, 21, 31 * 4);
const signature = Buffer.from("0000000c4a584c200d0a870a00000014667479706a786c20000000006a786c20", "hex");
const source = jxlBoxes(bare).length
  ? Buffer.concat([bare, ...box("wxyz", Uint8Array.of(2, 7, 1))])
  : Buffer.concat([signature, ...box("jxlc", bare), ...box("wxyz", Uint8Array.of(2, 7, 1))]);
const svg = '\uFEFF<svg xmlns="http://www.w3.org/2000/svg" width="25" height="17">\r\n' +
  '  <rect width="25" height="17" fill="#9dc1ff"/>\r\n' +
  '  <script>parent.postMessage("svg-executed", "*")</script>\r\n' +
  '  <image href="/unexpected-svg-request.png" width="5" height="5"/>\r\n</svg>';
const replacement = svg.replace(/\r\n/g, "\n").replace("#9dc1ff", "#ee1166");

const browser = await chromium.launch({ args: ["--disable-dev-shm-usage"] });
try {
  const page = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
  const errors = [];
  const unexpected = [];
  page.on("pageerror", error => errors.push(error.message));
  page.on("request", request => { if (request.url().endsWith("unexpected-svg-request.png")) unexpected.push(request.url()); });
  await page.addInitScript(() => {
    globalThis.svgExecuted = false;
    addEventListener("message", event => { if (event.data === "svg-executed") globalThis.svgExecuted = true; });
  });
  await page.goto(baseUrl);
  await page.getByText("Relaxed SIMD", { exact: true }).waitFor();
  const open = page.locator("#open-file");
  const status = page.locator("#status");
  async function waitStatus(value) {
    await status.filter({ hasText: value }).waitFor();
    await page.waitForFunction(() => !document.getElementById("save").disabled);
  }
  async function chooseLayers(button, file) {
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: button, exact: true }).click();
    await (await chooser).setFiles(file);
  }
  async function capture(button, path) {
    const downloaded = page.waitForEvent("download");
    await page.getByRole("button", { name: button, exact: true }).click();
    const file = await downloaded;
    if (button === "Save project") assert.match(file.suggestedFilename(), /\.project\.jxl$/);
    if (button === "Export JXL") assert.match(file.suggestedFilename(), /\.jxl$/);
    await file.saveAs(path);
    return await readFile(path);
  }

  await chooseLayers("Add layers", { name: "gradient.jxl", mimeType: "image/jxl", buffer: source });
  await waitStatus("Image opened");
  const unedited = await capture("Save project", `${output}/unedited.project.jxl`);
  assert.deepEqual(jxlBoxes(unedited).find(box => box.type === "jxlc").data,
    jxlBoxes(source).find(box => box.type === "jxlc").data);
  assert.equal(opaqueBoxes(unedited).length, 1);
  await chooseLayers("Add layers", { name: "overlay.jxl", mimeType: "image/jxl", buffer: Buffer.from(second) });
  await waitStatus("Layer added");
  await chooseLayers("Add layers from the Layers panel", { name: "vector.svg", mimeType: "image/svg+xml", buffer: Buffer.from(svg) });
  await waitStatus("Layer added");
  assert.equal(await page.locator(".layer").count(), 3);
  assert.equal(await page.evaluate(() => svgExecuted), false);
  assert.deepEqual(unexpected, []);

  await page.getByRole("button", { name: "Edit SVG source" }).click();
  await page.getByRole("textbox", { name: "SVG markup" }).fill(replacement);
  await page.getByRole("button", { name: "Apply SVG" }).click();
  await waitStatus("SVG rendered");
  assert.equal(await page.locator("dialog").evaluate(dialog => dialog.open), false);
  await page.getByRole("button", { name: "Lower", exact: true }).click();
  await page.getByRole("button", { name: "Hide overlay.jxl" }).click();
  await page.getByRole("button", { name: "Undo", exact: true }).click();
  assert.equal(await page.getByRole("button", { name: "Hide overlay.jxl" }).count(), 1);
  await page.getByRole("button", { name: "Redo", exact: true }).click();
  assert.equal(await page.getByRole("button", { name: "Show overlay.jxl" }).count(), 1);

  const canvas = page.locator("#canvas");
  await canvas.focus();
  await page.keyboard.press("Shift+ArrowRight");
  await page.keyboard.press("ArrowDown");
  const previousPos = Number(await page.locator("#position-x").inputValue());
  const bounds = await canvas.boundingBox();
  const x = bounds.x + (previousPos + 3) * bounds.width / 127;
  const y = bounds.y + (Number(await page.locator("#position-y").inputValue()) + 3) * bounds.height / 93;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + bounds.width * 4 / 127, y, { steps: 4 });
  await page.mouse.up();
  assert.equal(Number(await page.locator("#position-x").inputValue()), previousPos + 4);
  await page.locator("#adjust-opacity").press("ArrowLeft");
  await page.locator("#adjust-exposure").press("ArrowRight");
  await page.getByRole("button", { name: "History", exact: true }).click();
  assert.equal(await page.getByRole("button", { name: "Edited SVG source", exact: true }).count(), 1);
  await page.getByRole("button", { name: "Adjustments", exact: true }).click();

  const preview = () => canvas.evaluate(el => Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data));
  // Wait for the final queued paint so the actual canvas is the pixel oracle.
  await page.evaluate(() => new Promise(requestAnimationFrame));
  await page.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
  const expected = Buffer.from(await preview());
  const saved = await capture("Save project", `${output}/gradient.project.jxl`);
  await waitStatus("Project saved");
  await writeFile(`${output}/gradient.rgba`, expected);
  const project = readProject(saved);
  assert.deepEqual(project.manifest.layers.map(layer => layer.name), ["gradient.jxl", "vector.svg", "overlay.jxl"]);
  assert.equal(project.manifest.layers[2].visible, false);
  assert.equal(project.manifest.layers[1].opacity, 0.99);
  assert.equal(project.manifest.layers[1].exposure, 0.01);
  assert.equal(project.manifest.filterEngine, "zenfilters-v1");
  assert.deepEqual(Buffer.from(project.assets.get(project.manifest.layers[0].id).source), source);
  assert.deepEqual(Buffer.from(project.assets.get(project.manifest.layers[2].id).source), Buffer.from(second));
  const vector = project.assets.get(project.manifest.layers[1].id);
  assert.deepEqual(Buffer.from(vector.source), Buffer.from(svg));
  assert.deepEqual(Buffer.from(vector.svgEdit), Buffer.from(replacement));
  const render = decode_jxl(vector.rendition);
  assert.deepEqual([render.width(), render.height()], [25, 17]);
  assert.deepEqual([...render.into_pixels().subarray(0, 4)], [238, 17, 102, 255]);
  assert.deepEqual(Buffer.from(decode_jxl(saved).into_pixels()), expected);
  assert.deepEqual([...project.preserved[0].data], [2, 7, 1]);

  await page.reload();
  await page.getByText("Relaxed SIMD", { exact: true }).waitFor();
  await open.setInputFiles({ name: "gradient.project.jxl", mimeType: "image/jxl", buffer: saved });
  await waitStatus("Project reopened");
  await page.evaluate(() => new Promise(requestAnimationFrame));
  await page.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
  assert.deepEqual(Buffer.from(await preview()), expected);
  assert.equal(await page.locator(".layer").count(), 3);
  await page.getByRole("button", { name: "vector.svg SVG" }).click();
  await page.getByRole("button", { name: "Edit SVG source" }).click();
  assert.equal(await page.getByRole("textbox", { name: "SVG markup" }).inputValue(), replacement);
  await page.getByRole("button", { name: "Cancel" }).click();
  const flat = await capture("Export JXL", `${output}/gradient.jxl`);
  assert.equal(readProject(flat), null);
  assert.deepEqual([...opaqueBoxes(flat)[0].data], [2, 7, 1]);
  assert.deepEqual(Buffer.from(decode_jxl(flat).into_pixels()), expected);
  const originalSvg = await capture("Download source", `${output}/vector.svg`);
  assert.deepEqual(originalSvg, Buffer.from(svg));
  assert.deepEqual(errors, []);
  await page.screenshot({ path: `${output}/editor.png` });
  await page.close();

  const native = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
  const nativeBase = rgba(64, 41, () => [30, 70, 110, 255]);
  const nativeFinal = rgba(64, 41, (x, y) => x >= 9 && x < 14 && y >= 7 && y < 11 ? [220, 110, 55, 190] : [30, 70, 110, 255]);
  const nativeIds = "00000000-0000-4000-8000-000000000001,00000000-0000-4000-8000-000000000002";
  const nativeFile = encode_layers(Buffer.concat([nativeBase, nativeFinal]), nativeFinal, 64, 41, nativeIds);
  assert.ok(nativeFile);
  const nativeInput = jxlBoxes(nativeFile).length
    ? Buffer.concat([nativeFile, ...box("wxyz", Uint8Array.of(2, 7, 1))])
    : Buffer.concat([signature, ...box("jxlc", nativeFile), ...box("wxyz", Uint8Array.of(2, 7, 1))]);
  await native.goto(baseUrl);
  await native.locator("#open-file").setInputFiles({ name: "native.jxl", mimeType: "image/jxl", buffer: nativeInput });
  await native.getByText("Image opened", { exact: true }).waitFor();
  await native.getByRole("button", { name: "Native JXL", exact: true }).click();
  await native.getByText("2 native JPEG XL frames", { exact: true }).waitFor();
  await native.getByLabel("Source frame").selectOption("1");
  await native.getByText("5 × 4 at (9, 7) · Replace", { exact: true }).waitFor();
  const info = JSON.parse(await native.locator("#native-frame-json").textContent());
  assert.deepEqual(info.origin, [9, 7]);
  assert.deepEqual(info.size, [5, 4]);
  assert.equal(info.colorBlend.sourceReference, 1);
  await native.getByLabel("Preview channel").selectOption({ label: "Alpha" });
  const rawDownload = native.waitForEvent("download");
  await native.getByRole("button", { name: "Download F32 samples" }).click();
  const raw = await rawDownload;
  assert.equal(raw.suggestedFilename(), "jxl-frame-2-plane-1.f32le");
  const sourceAlpha = await readFile(await raw.path());
  assert.equal(sourceAlpha.byteLength, 5 * 4 * 4);
  for (let i = 0; i < sourceAlpha.length; i += 4) {
    assert.equal(sourceAlpha.readFloatLE(i), Math.fround(190 / 255));
  }
  await native.getByText("Container boxes", { exact: true }).click();
  await native.getByLabel("Box", { exact: true }).selectOption({ label: "wxyz · 3 bytes" });
  const boxDownload = native.waitForEvent("download");
  await native.getByRole("button", { name: "Download box" }).click();
  const boxFile = await boxDownload;
  assert.deepEqual(await readFile(await boxFile.path()), Buffer.concat(box("wxyz", Uint8Array.of(2, 7, 1))));
  await native.screenshot({ path: `${output}/native.png` });
  await native.getByRole("button", { name: "Add frame as layer" }).click();
  await native.getByText("Source-local frame added to Layers", { exact: true }).waitFor();
  assert.equal(await native.locator("#position-x").inputValue(), "9");
  assert.equal(await native.locator("#position-y").inputValue(), "7");
  await native.evaluate(() => new Promise(requestAnimationFrame));
  const presentation = await native.locator("#canvas").evaluate(el =>
    Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data));
  await writeFile(`${output}/native.rgba`, Buffer.from(presentation));
  await writeFile(`${output}/native-source.rgba`, nativeFinal);
  const nativeDownload = native.waitForEvent("download");
  await native.getByRole("button", { name: "Save project", exact: true }).click();
  const nativeSave = await nativeDownload;
  assert.equal(nativeSave.suggestedFilename(), "native.project.jxl");
  const nativeProject = await readFile(await nativeSave.path());
  assert.deepEqual(Buffer.from(decode_jxl(nativeProject).into_pixels()), Buffer.from(presentation));
  const nativeManifest = readProject(nativeProject);
  const extracted = nativeManifest.manifest.layers[1];
  assert.equal(extracted.nativeFrame, 1);
  assert.deepEqual(Buffer.from(nativeManifest.assets.get(extracted.id).source), nativeInput);
  await native.reload();
  await native.locator("#open-file").setInputFiles({
    name: nativeSave.suggestedFilename(), mimeType: "image/jxl", buffer: nativeProject,
  });
  await native.getByText("Project reopened · original assets restored", { exact: true }).waitFor();
  assert.equal(await native.locator("#position-x").inputValue(), "9");
  assert.equal(await native.locator("#position-y").inputValue(), "7");
  await writeFile(`${output}/native-source.jxl`, nativeInput);
  await writeFile(`${output}/native.project.jxl`, nativeProject);
  await writeFile(`${output}/native-alpha.f32le`, sourceAlpha);
  await native.close();

  const photo = await browser.newPage({ viewport: { width: 1400, height: 940 } });
  await photo.goto(baseUrl);
  await photo.getByRole("button", { name: "Try a sample" }).click();
  await photo.locator("#status").filter({ hasText: "Image opened" }).waitFor();
  assert.equal(await photo.locator("#dimensions").textContent(), "1296 × 864 px");
  // Single-layer photos must immediately display on-canvas resize & rotate handles.
  await photo.locator("#outline .handle-se").waitFor({ state: "visible" });
  await photo.locator("#outline .handle-rotate").waitFor({ state: "visible" });
  assert.equal(await photo.locator("#outline .handle-se").evaluate(el => getComputedStyle(el).touchAction), "none");
  assert.equal(await photo.locator("#outline .handle-rotate").evaluate(el => getComputedStyle(el).touchAction), "none");
  await photo.screenshot({ path: `${output}/photo.png` });
  // Regression guard: Auto Tone + Auto Contrast on 1296x864 (> 202-row strip_height)
  // must process the full frame without horizontal banding at strip boundaries.
  await photo.waitForFunction(() => Boolean(document.getElementById("zf-autoTone")));
  await photo.locator("#zf-autoTone").click();
  await photo.locator("#zf-autoContrast").click();
  await photo.evaluate(() => new Promise(requestAnimationFrame));
  await photo.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
  await photo.screenshot({ path: `${output}/photo-auto-tone.png` });
  // Reset back to neutral before mobile screenshot
  await photo.locator("#zf-autoTone").click();
  await photo.locator("#zf-autoContrast").click();
  await photo.evaluate(() => new Promise(requestAnimationFrame));
  await photo.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
  await photo.setViewportSize({ width: 390, height: 844 });
  assert.equal(await photo.evaluate(() => document.documentElement.scrollWidth), 390);
  await photo.screenshot({ path: `${output}/mobile.png`, fullPage: true });
  await photo.close();

  // ── Native JPEG XL Timeline (Animation): create, edit per-frame, play, save/reopen, export ──
  {
    const tl = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const tlErrors = [];
    tl.on("pageerror", error => tlErrors.push(error.message));
    await tl.goto(baseUrl);
    await tl.getByText("Relaxed SIMD", { exact: true }).waitFor();
    async function waitReady(textMatch) {
      if (textMatch) await tl.locator("#status").filter({ hasText: textMatch }).waitFor();
      await tl.waitForFunction(() => !document.getElementById("save").disabled);
    }
    async function dl(btnName, savePath, extPattern) {
      const ev = tl.waitForEvent("download");
      await tl.getByRole("button", { name: btnName, exact: true }).click();
      const file = await ev;
      assert.match(file.suggestedFilename(), extPattern);
      await file.saveAs(savePath);
      return await readFile(savePath);
    }
    await tl.locator("#open-file").setInputFiles({ name: "anim-base.jxl", mimeType: "image/jxl", buffer: source });
    await waitReady("Image opened");
    const addChooser = tl.waitForEvent("filechooser");
    await tl.getByRole("button", { name: "Add layers", exact: true }).click();
    await (await addChooser).setFiles({ name: "overlay.jxl", mimeType: "image/jxl", buffer: Buffer.from(second) });
    await waitReady("Layer added");

    await tl.getByRole("button", { name: "Timeline", exact: true }).click();
    await tl.locator("#timeline-loops").fill("2");
    await tl.locator("#timeline-loops").dispatchEvent("change");
    await tl.locator("#frame-name").fill("Intro");
    await tl.locator("#frame-name").dispatchEvent("change");
    await tl.locator("#frame-duration").fill("150");
    await tl.locator("#frame-duration").dispatchEvent("change");
    await tl.locator("#frame-timecode").fill("100");
    await tl.locator("#frame-timecode").dispatchEvent("change");

    // Add frame 2 and move overlay independently on frame 2
    await tl.getByRole("button", { name: "+ Frame", exact: true }).click();
    await tl.locator("#frame-name").fill("Shift");
    await tl.locator("#frame-name").dispatchEvent("change");
    await tl.locator("#frame-duration").fill("250");
    await tl.locator("#frame-duration").dispatchEvent("change");
    await tl.locator("#frame-timecode").fill("200");
    await tl.locator("#frame-timecode").dispatchEvent("change");
    await tl.locator("#position-x").fill("12");
    await tl.locator("#position-x").dispatchEvent("change");
    await tl.locator("#position-y").fill("8");
    await tl.locator("#position-y").dispatchEvent("change");

    // Add frame 3 and hide overlay on frame 3 only
    await tl.getByRole("button", { name: "+ Frame", exact: true }).click();
    await tl.locator("#frame-name").fill("Clean");
    await tl.locator("#frame-name").dispatchEvent("change");
    await tl.locator("#frame-duration").fill("350");
    await tl.locator("#frame-duration").dispatchEvent("change");
    await tl.locator("#frame-timecode").fill("300");
    await tl.locator("#frame-timecode").dispatchEvent("change");
    await tl.getByRole("button", { name: "Hide overlay.jxl" }).click();

    // Capture exact expected pixels of each frame directly from the editor canvas
    const readCanvas = () => tl.locator("#canvas").evaluate(el =>
      Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data));
    const frameButtons = tl.locator(".timeline-frame");
    assert.equal(await frameButtons.count(), 3);
    const expectedFrames = [];
    for (let i = 0; i < 3; i++) {
      await frameButtons.nth(i).click();
      await tl.evaluate(() => new Promise(requestAnimationFrame));
      const px = Buffer.from(await readCanvas());
      expectedFrames.push(px);
      await writeFile(`${output}/timeline-f${i}.rgba`, px);
    }
    assert.notDeepEqual(expectedFrames[0], expectedFrames[1]);
    assert.notDeepEqual(expectedFrames[1], expectedFrames[2]);

    // Verify Play advances frames, then Pause
    await frameButtons.nth(0).click();
    await tl.getByRole("button", { name: "Play", exact: true }).click();
    await tl.waitForFunction(() => document.getElementById("timeline-counter").textContent !== "Frame 1 / 3");
    await tl.getByRole("button", { name: "Pause", exact: true }).click();
    await tl.screenshot({ path: `${output}/timeline.png` });

    // Verify 390px mobile viewport with Timeline dock open has no horizontal overflow
    await tl.setViewportSize({ width: 390, height: 844 });
    assert.equal(await tl.evaluate(() => document.documentElement.scrollWidth), 390);
    await tl.setViewportSize({ width: 1400, height: 940 });

    const tlProject = await dl("Save project", `${output}/timeline.project.jxl`, /\.project\.jxl$/);
    await waitReady("Project saved");
    const tlInfo = JSON.parse(native_jxl_info(tlProject));
    assert.equal(tlInfo.animation.ticksNumerator, 1000);
    assert.equal(tlInfo.animation.loopCount, 2);
    assert.equal(tlInfo.animation.hasTimecodes, true);
    assert.equal(tlInfo.frames.length, 3);
    assert.deepEqual(tlInfo.frames.map(f => f.name), ["Intro", "Shift", "Clean"]);
    assert.deepEqual(tlInfo.frames.map(f => f.durationTicks), [150, 250, 350]);
    assert.deepEqual(tlInfo.frames.map(f => f.timecode), [100, 200, 300]);
    const decodedTl = native_jxl_timeline(tlProject);
    const decodedPx = decodedTl.into_pixels();
    const fBytes = 127 * 93 * 4;
    for (let i = 0; i < 3; i++) {
      assert.deepEqual(Buffer.from(decodedPx.subarray(i * fBytes, (i + 1) * fBytes)), expectedFrames[i]);
    }

    // Reopen .project.jxl and verify per-frame editable state survived
    await tl.reload();
    await tl.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await tl.locator("#open-file").setInputFiles({ name: "timeline.project.jxl", mimeType: "image/jxl", buffer: tlProject });
    await waitReady("Project reopened");
    assert.equal(await tl.locator(".timeline-frame").count(), 3);
    await tl.locator(".timeline-frame").nth(1).click();
    assert.equal(await tl.locator("#position-x").inputValue(), "12");
    assert.equal(await tl.locator("#position-y").inputValue(), "8");
    assert.equal(await tl.locator("#frame-duration").inputValue(), "250");
    await tl.locator(".timeline-frame").nth(2).click();
    assert.equal(await tl.getByRole("button", { name: "Show overlay.jxl" }).count(), 1);

    // Export standalone animated .jxl and reopen it directly as an editable timeline
    const tlFlat = await dl("Export JXL", `${output}/timeline.jxl`, /\.jxl$/);
    assert.equal(readProject(tlFlat), null);
    const flatInfo = JSON.parse(native_jxl_info(tlFlat));
    assert.equal(flatInfo.frames.length, 3);
    assert.deepEqual(flatInfo.frames.map(f => f.durationTicks), [150, 250, 350]);
    await tl.reload();
    await tl.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await tl.locator("#open-file").setInputFiles({ name: "timeline.jxl", mimeType: "image/jxl", buffer: tlFlat });
    await waitReady("Image opened");
    assert.equal(await tl.locator(".timeline-frame").count(), 3);
    assert.equal(await tl.locator("#timeline-loops").inputValue(), "2");
    assert.deepEqual(tlErrors, []);
    await tl.close();
  }

  // ── Native JPEG XL Manual Splines: author, preview, save/reopen, export + bitstream flag check ──
  {
    const sp = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const spErrors = [];
    sp.on("pageerror", error => spErrors.push(error.message));
    const darkBg = rgba(96, 64, () => [20, 26, 38, 255]);
    const darkJxl = encode_jxl(darkBg, 96, 64, 96 * 4);
    await sp.goto(baseUrl);
    await sp.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sp.locator("#open-file").setInputFiles({ name: "dark.jxl", mimeType: "image/jxl", buffer: Buffer.from(darkJxl) });
    await sp.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    await sp.getByRole("button", { name: "Splines", exact: true }).click();
    await sp.getByText("Manual JPEG XL splines require VarDCT lossy coding.").waitFor();
    await sp.getByRole("button", { name: "Switch to VarDCT still", exact: true }).click();
    await sp.getByRole("button", { name: "+ Add spline", exact: true }).click();
    await sp.locator("#status").filter({ hasText: "Manual JPEG XL spline added" }).waitFor();
    await sp.locator("#spline-points").fill("12, 32\n48, 16\n84, 32");
    await sp.locator("#spline-points").dispatchEvent("change");
    await sp.evaluate(() => new Promise(requestAnimationFrame));
    await sp.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    // Click canvas to append 4th control point
    await sp.getByRole("button", { name: "Click canvas to add points", exact: true }).click();
    const boxBounds = await sp.locator("#canvas").boundingBox();
    await sp.mouse.click(boxBounds.x + boxBounds.width * 70 / 96, boxBounds.y + boxBounds.height * 48 / 64);
    await sp.waitForFunction(() => document.getElementById("spline-points").value.trim().split("\n").length === 4);
    await sp.evaluate(() => new Promise(requestAnimationFrame));
    await sp.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    // Verify canvas live preview visibly shows yellow spline at midpoint (48, 16)
    const previewMid = await sp.locator("#canvas").evaluate(el => {
      const px = Math.round(48 * el.width / 96);
      const py = Math.round(16 * el.height / 64);
      return Array.from(el.getContext("2d").getImageData(px, py, 1, 1).data);
    });
    assert.ok(previewMid[0] > 140 && previewMid[1] > 120, `expected bright yellow spline preview, got ${previewMid}`);
    await sp.screenshot({ path: `${output}/splines.png` });

    const spSaveEv = sp.waitForEvent("download");
    await sp.getByRole("button", { name: "Save project", exact: true }).click();
    const spSaveFile = await spSaveEv;
    assert.match(spSaveFile.suggestedFilename(), /\.project\.jxl$/);
    await spSaveFile.saveAs(`${output}/splines.project.jxl`);
    const spProject = await readFile(`${output}/splines.project.jxl`);
    const spInfo = JSON.parse(native_jxl_info(spProject));
    assert.equal(spInfo.frames[0].coding.encoding, "VarDCT");
    assert.equal(spInfo.frames[0].coding.flags.splines, true);
    assert.ok(spInfo.vardctQuantizer?.globalScale >= 1);

    // Reopen .project.jxl and verify spline remains editable with 4 control points
    await sp.reload();
    await sp.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sp.locator("#open-file").setInputFiles({ name: "splines.project.jxl", mimeType: "image/jxl", buffer: spProject });
    await sp.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await sp.getByRole("button", { name: "Splines", exact: true }).click();
    assert.equal((await sp.locator("#spline-points").inputValue()).trim().split("\n").length, 4);

    const spExpEv = sp.waitForEvent("download");
    await sp.getByRole("button", { name: "Export JXL", exact: true }).click();
    const spExpFile = await spExpEv;
    assert.match(spExpFile.suggestedFilename(), /\.jxl$/);
    await spExpFile.saveAs(`${output}/splines.jxl`);
    const spFlat = await readFile(`${output}/splines.jxl`);
    const spFlatInfo = JSON.parse(native_jxl_info(spFlat));
    assert.equal(spFlatInfo.frames[0].coding.flags.splines, true);
    assert.deepEqual(spErrors, []);
    await sp.close();
  }

  // ── Coding Modes (VarDCT Progressive/LF/Noise/Resampling + Modular Group/Squeeze/RCT) & CMYK ──
  {
    const cm = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const cmErrors = [];
    cm.on("pageerror", error => cmErrors.push(error.message));
    const grad = rgba(64, 48, (x, y) => [(x * 4) % 256, (y * 5) % 256, ((x + y) * 3) % 256, 255]);
    const gradJxl = encode_jxl(grad, 64, 48, 64 * 4);
    await writeFile(`${output}/coding-modular.rgba`, grad);
    await cm.goto(baseUrl);
    await cm.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await cm.locator("#open-file").setInputFiles({ name: "coding.jxl", mimeType: "image/jxl", buffer: Buffer.from(gradJxl) });
    await cm.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    await cm.getByRole("button", { name: "Coding", exact: true }).click();

    // VarDCT progressive AC + LF frame + photon noise + gaborish off + EPF 0
    await cm.locator("#coding-mode").selectOption("vardct");
    await cm.locator("#coding-progressive").selectOption("dc_vlf_lf_ac");
    await cm.locator("#coding-progressive-dc").selectOption("1");
    await cm.locator("#coding-noise-mode").selectOption("photon_iso");
    await cm.locator("#coding-photon-iso").fill("3200");
    await cm.locator("#coding-photon-iso").dispatchEvent("change");
    await cm.locator("#coding-gaborish").selectOption("off");
    await cm.locator("#coding-epf").selectOption("0");
    let dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/coding-vardct.jxl`);
    const vInfo = JSON.parse(native_jxl_info(await readFile(`${output}/coding-vardct.jxl`)));
    assert.equal(vInfo.frames[0].coding.encoding, "VarDCT");
    assert.equal(vInfo.frames[0].coding.numPasses, 3);
    assert.equal(vInfo.frames[0].coding.flags.useLfFrame, true);
    assert.equal(vInfo.frames[0].coding.flags.noise, true);
    assert.equal(vInfo.frames[0].coding.gaborish, false);
    assert.equal(vInfo.frames[0].coding.epfIters, 0);
    assert.ok(vInfo.vardctQuantizer?.globalScale >= 1);

    // VarDCT spatial resampling 2x
    await cm.locator("#coding-progressive-dc").selectOption("0");
    await cm.locator("#coding-resampling").selectOption("2");
    dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/coding-resamp.jxl`);
    const rInfo = JSON.parse(native_jxl_info(await readFile(`${output}/coding-resamp.jxl`)));
    assert.equal(rInfo.frames[0].coding.upsampling, 2);

    // Modular squeeze + group size 128 + YCoCg RCT + save/reopen persistence
    await cm.locator("#coding-mode").selectOption("modular");
    await cm.locator("#coding-squeeze").check();
    await cm.locator("#coding-group-size").selectOption("0");
    await cm.locator("#coding-rct").selectOption("ycocg");
    dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/coding-modular.jxl`);
    const modBytes = await readFile(`${output}/coding-modular.jxl`);
    const mInfo = JSON.parse(native_jxl_info(modBytes));
    assert.equal(mInfo.frames[0].coding.encoding, "Modular");
    assert.equal(mInfo.frames[0].coding.modularGroupSize, 128);
    assert.deepEqual(Buffer.from(decode_jxl(modBytes).into_pixels()), Buffer.from(grad));

    // CMYK lossless exact round-trip + save/reopen + lossy CMYK export
    const icc = generate_minimal_cmyk_icc();
    const cmykSwatches = new Uint8Array(24 * 16 * 4);
    const pal = [[255,255,255,255],[0,255,255,255],[255,0,255,255],[255,255,0,255],[255,255,255,0],[64,128,192,224]];
    for (let y = 0; y < 16; y++) for (let x = 0; x < 24; x++) {
      cmykSwatches.set(pal[Math.floor(y / 8) * 3 + Math.floor(x / 8)], (y * 24 + x) * 4);
    }
    const cmykSrc = encode_cmyk_jxl(cmykSwatches, 24, 16, 8, icc, JSON.stringify({ mode: "modular", effort: 3 }));
    await writeFile(`${output}/cmyk-lossless.cmyk8`, cmykSwatches);
    await writeFile(`${output}/cmyk-expected.icc`, icc);

    await cm.reload();
    await cm.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await cm.locator("#open-file").setInputFiles({ name: "swatches-cmyk.jxl", mimeType: "image/jxl", buffer: Buffer.from(cmykSrc) });
    await cm.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    await cm.getByRole("button", { name: "Coding", exact: true }).click();
    assert.equal(await cm.locator("#coding-color-model").inputValue(), "cmyk");
    assert.equal(await cm.locator("#cmyk-bits").inputValue(), "8");
    assert.equal(await cm.locator("#cmyk-mode").inputValue(), "modular");
    await cm.getByText("C=255 M=255 Y=255 K=255").waitFor();

    dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Save project", exact: true }).click();
    const cmykProjFile = await dlEv;
    assert.equal(cmykProjFile.suggestedFilename(), "swatches-cmyk.project.jxl");
    await cmykProjFile.saveAs(`${output}/cmyk-lossless.project.jxl`);
    const cmykProjBytes = await readFile(`${output}/cmyk-lossless.project.jxl`);

    // Reopen CMYK .project.jxl and confirm exact CMYK samples & ICC survive
    await cm.reload();
    await cm.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await cm.locator("#open-file").setInputFiles({ name: "swatches-cmyk.project.jxl", mimeType: "image/jxl", buffer: cmykProjBytes });
    await cm.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await cm.getByRole("button", { name: "Coding", exact: true }).click();
    assert.equal(await cm.locator("#coding-color-model").inputValue(), "cmyk");
    await cm.getByText("C=255 M=255 Y=255 K=255").waitFor();

    dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/cmyk-lossless.jxl`);
    const cmykOutBytes = await readFile(`${output}/cmyk-lossless.jxl`);
    const cmykOutInfo = JSON.parse(native_jxl_info(cmykOutBytes));
    assert.equal(cmykOutInfo.codestreamLevel, 10);
    assert.equal(cmykOutInfo.profile.cmyk, true);
    assert.equal(cmykOutInfo.extraChannels[0].type, "Black");
    const ext = native_extract_cmyk(cmykOutBytes);
    assert.deepEqual(ext.take_samples(), cmykSwatches);
    assert.deepEqual(ext.take_icc(), icc);
    ext.free();

    // Switch CMYK to VarDCT lossy (ICC -> linear sRGB XYB + preserved Black extra channel)
    await cm.locator("#cmyk-mode").selectOption("vardct");
    await cm.getByText(/converts CMY\/K via the embedded ICC to linear sRGB XYB/).waitFor();
    dlEv = cm.waitForEvent("download");
    await cm.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/cmyk-lossy.jxl`);
    const cmykLossyBytes = await readFile(`${output}/cmyk-lossy.jxl`);
    const cmykLossyInfo = JSON.parse(native_jxl_info(cmykLossyBytes));
    assert.equal(cmykLossyInfo.codestreamLevel, 10);
    assert.equal(cmykLossyInfo.xybEncoded, true);
    assert.equal(cmykLossyInfo.extraChannels[0].type, "Black");
    assert.deepEqual(cmErrors, []);
    await cm.close();
  }

  // ── Phase 2: Gallery, Rich SVG Studio, Crisp Vector Scaling, ZenFilters, ZenResize & On-Canvas Handles ──
  {
    const st = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const stErrors = [];
    st.on("pageerror", error => stErrors.push(error.message));
    await st.goto(baseUrl);
    await st.getByText("Relaxed SIMD", { exact: true }).waitFor();
    async function waitReady(textMatch) {
      if (textMatch) await st.locator("#status").filter({ hasText: textMatch }).waitFor();
      await st.waitForFunction(() => !document.getElementById("save").disabled);
    }

    // 1. Open calibrated Sunset Color Chart from Prefilled Gallery (zero disk upload)
    await st.getByRole("button", { name: "Browse gallery", exact: true }).click();
    await st.locator("#gallery-dialog").waitFor({ state: "visible" });
    await st.screenshot({ path: `${output}/gallery.png` });
    await st.locator('[data-gallery-id="color-checker"] button[data-action="add"]').click();
    await waitReady("Image opened");
    assert.equal(await st.locator("#dimensions").textContent(), "512 × 340 px");

    // 2. Add Hero Title Banner SVG from Prefilled Gallery
    await st.locator("#add-gallery").click();
    await st.locator('[data-gallery-cat="vectors"]').click();
    await st.locator('[data-gallery-id="svg-title"] button[data-action="add"]').click();
    await waitReady("Layer added");
    assert.equal(await st.locator(".layer").count(), 2);

    // 3. Edit SVG visually in SVG Vector Studio (live inert preview + text edit + add circle)
    await st.getByRole("button", { name: "Edit SVG source", exact: true }).click();
    await st.waitForFunction(() => {
      const img = document.getElementById("svg-preview");
      return img && img.complete && img.naturalWidth > 0;
    });
    // Select the first <text> element and edit its copy visually
    const textOptValue = await st.locator("#svg-el-select option").evaluateAll(opts => {
      const found = opts.find(o => o.textContent.includes("<text>"));
      return found ? found.value : null;
    });
    assert.ok(textOptValue !== null, "expected <text> element in SVG inspector");
    await st.locator("#svg-el-select").selectOption(textOptValue);
    await st.locator("#svg-el-text").fill("ZEN STUDIO LIVE");
    await st.locator("#svg-add-circle").click();
    await st.screenshot({ path: `${output}/svg-studio.png` });
    assert.ok((await st.locator("#svg-source").inputValue()).includes("ZEN STUDIO LIVE"));
    assert.ok((await st.locator("#svg-source").inputValue()).includes("<circle"));
    await st.getByRole("button", { name: "Apply SVG", exact: true }).click();
    await waitReady("SVG rendered");

    // 4. Drag SE corner resize handle directly on canvas to scale SVG crisply
    const seHandle = st.locator("#outline .handle-se");
    await seHandle.waitFor({ state: "visible" });
    const hBox = await seHandle.boundingBox();
    await st.mouse.move(hBox.x + hBox.width / 2, hBox.y + hBox.height / 2);
    await st.mouse.down();
    await st.mouse.move(hBox.x + hBox.width / 2 - 48, hBox.y + hBox.height / 2 - 14, { steps: 5 });
    await st.mouse.up();
    const svgScale = Number(await st.locator("#adjust-scale").inputValue());
    assert.ok(svgScale !== 1, `expected SE handle drag to change scale, got ${svgScale}`);
    assert.match(await st.locator("#scaled-dimensions").textContent(), /Vector SVG re-rasterized crisply/);

    // 5. Select base raster layer, apply ZenFilters (Auto Tone + Preset + Film Look + Clarity + Vignette) + zenresize Lanczos
    await st.locator(".layer-pick").last().click();
    await st.waitForFunction(() => Boolean(document.getElementById("zf-preset")));
    const readCanvas = () => st.locator("#canvas").evaluate(el =>
      Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data));
    await st.evaluate(() => new Promise(requestAnimationFrame));
    await st.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    const beforeFilters = Buffer.from(await readCanvas());

    await st.locator("#zf-autoTone").click();
    await st.locator("#zf-preset").selectOption("Golden Hour");
    await st.locator("#zf-film-look").selectOption("portra");
    await st.locator('[data-zf-group="detail"] summary').click();
    await st.locator("#zf-clarity").fill("0.4");
    await st.locator("#zf-clarity").dispatchEvent("input");
    await st.locator("#zf-clarity").dispatchEvent("change");
    await st.locator("#resize-kernel").selectOption("lanczos");
    await st.locator("#adjust-scale").fill("0.85");
    await st.locator("#adjust-scale").dispatchEvent("input");
    await st.locator("#adjust-scale").dispatchEvent("change");
    assert.match(await st.locator("#scaled-dimensions").textContent(), /zenresize:.*lanczos, linear/);
    await st.evaluate(() => new Promise(requestAnimationFrame));
    await st.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    const afterFilters = Buffer.from(await readCanvas());
    assert.notDeepEqual(afterFilters, beforeFilters, "ZenFilters + zenresize must visibly transform canvas");
    await st.screenshot({ path: `${output}/zenfilters-svg.png` });

    // Save project + verify round-trip + export flat JXL
    await writeFile(`${output}/studio-rich.rgba`, afterFilters);
    let dlEv = st.waitForEvent("download");
    await st.getByRole("button", { name: "Save project", exact: true }).click();
    await (await dlEv).saveAs(`${output}/studio-rich.project.jxl`);
    await waitReady("Project saved");
    const richProj = await readFile(`${output}/studio-rich.project.jxl`);
    assert.deepEqual(Buffer.from(decode_jxl(richProj).into_pixels()), afterFilters);
    const parsedRich = readProject(richProj);
    assert.equal(parsedRich.manifest.layers[0].filters.preset, "Golden Hour");
    assert.equal(parsedRich.manifest.layers[0].filters.filmLook, "portra");
    assert.equal(parsedRich.manifest.layers[0].resizeKernel, "lanczos");

    await st.reload();
    await st.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await st.locator("#open-file").setInputFiles({ name: "studio-rich.project.jxl", mimeType: "image/jxl", buffer: richProj });
    await waitReady("Project reopened");
    await st.evaluate(() => new Promise(requestAnimationFrame));
    await st.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    assert.deepEqual(Buffer.from(await readCanvas()), afterFilters);
    await st.locator(".layer-pick").last().click();
    assert.equal(await st.locator("#zf-preset").inputValue(), "Golden Hour");
    assert.equal(await st.locator("#zf-film-look").inputValue(), "portra");
    assert.equal(await st.locator("#resize-kernel").inputValue(), "lanczos");

    dlEv = st.waitForEvent("download");
    await st.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/studio-rich.jxl`);
    assert.deepEqual(Buffer.from(decode_jxl(await readFile(`${output}/studio-rich.jxl`)).into_pixels()), afterFilters);
    assert.deepEqual(stErrors, []);
    await st.close();
  }

  // ── Phase 2: Non-Destructive Layer Masks (Parametric + Paintable Brush + Ruby Overlay + znmk Round-Trip) ──
  {
    const mk = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const mkErrors = [];
    mk.on("pageerror", error => mkErrors.push(error.message));
    const basePx = rgba(96, 64, () => [20, 40, 180, 255]);
    const topPx = rgba(96, 64, () => [240, 190, 30, 255]);
    const baseJxl = encode_jxl(basePx, 96, 64, 96 * 4);
    const topJxl = encode_jxl(topPx, 96, 64, 96 * 4);
    await mk.goto(baseUrl);
    await mk.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await mk.locator("#open-file").setInputFiles({ name: "mask-base.jxl", mimeType: "image/jxl", buffer: Buffer.from(baseJxl) });
    await mk.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    const chooser = mk.waitForEvent("filechooser");
    await mk.getByRole("button", { name: "Add layers", exact: true }).click();
    await (await chooser).setFiles({ name: "mask-top.jxl", mimeType: "image/jxl", buffer: Buffer.from(topJxl) });
    await mk.locator("#status").filter({ hasText: "Layer added" }).waitFor();

    await mk.getByRole("button", { name: "Mask", exact: true }).click();
    await mk.locator("#mask-enabled").check();
    await mk.locator("#mask-shape").selectOption("radial");
    await mk.evaluate(() => new Promise(requestAnimationFrame));
    await mk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));

    // Corner should be masked out (revealing blue base), center should remain yellow top
    const samplePx = (x, y) => mk.locator("#canvas").evaluate((el, [px, py]) => {
      const cx = Math.round(px * el.width / 96);
      const cy = Math.round(py * el.height / 64);
      return Array.from(el.getContext("2d").getImageData(cx, cy, 1, 1).data);
    }, [x, y]);
    const corner1 = await samplePx(2, 2);
    const center1 = await samplePx(48, 32);
    assert.ok(corner1[2] > 140 && corner1[0] < 60, `radial mask corner should reveal blue base, got ${corner1}`);
    assert.ok(center1[0] > 200 && center1[1] > 160, `radial mask center should show yellow top, got ${center1}`);

    // Paint erase stroke directly in the center of the canvas
    await mk.locator("#mask-brush-erase").click();
    const cBox = await mk.locator("#canvas").boundingBox();
    await mk.mouse.move(cBox.x + cBox.width * 0.48, cBox.y + cBox.height * 0.5);
    await mk.mouse.down();
    await mk.mouse.move(cBox.x + cBox.width * 0.52, cBox.y + cBox.height * 0.5, { steps: 3 });
    await mk.mouse.up();
    await mk.locator("#mask-brush-stop").click();
    await mk.evaluate(() => new Promise(requestAnimationFrame));
    await mk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    const centerAfterErase = await samplePx(48, 32);
    assert.ok(centerAfterErase[2] > 140 && centerAfterErase[0] < 60, `erased center should reveal blue base, got ${centerAfterErase}`);

    // Verify ruby overlay visually works in preview, then turn off before saving
    await mk.locator("#mask-ruby").check();
    await mk.evaluate(() => new Promise(requestAnimationFrame));
    await mk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    await mk.screenshot({ path: `${output}/masks.png` });
    await mk.locator("#mask-ruby").uncheck();
    await mk.evaluate(() => new Promise(requestAnimationFrame));
    await mk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));

    const expectedMasked = Buffer.from(await mk.locator("#canvas").evaluate(el =>
      Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data)));
    await writeFile(`${output}/masked.rgba`, expectedMasked);

    let dlEv = mk.waitForEvent("download");
    await mk.getByRole("button", { name: "Save project", exact: true }).click();
    await (await dlEv).saveAs(`${output}/masked.project.jxl`);
    const maskedProj = await readFile(`${output}/masked.project.jxl`);
    assert.equal(jxlBoxes(maskedProj).filter(b => b.type === "znmk").length, 1);
    assert.equal(opaqueBoxes(maskedProj).length, 0);
    assert.deepEqual(Buffer.from(decode_jxl(maskedProj).into_pixels()), expectedMasked);

    // Reopen masked .project.jxl and confirm painted + parametric mask survives bit-for-bit
    await mk.reload();
    await mk.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await mk.locator("#open-file").setInputFiles({ name: "masked.project.jxl", mimeType: "image/jxl", buffer: maskedProj });
    await mk.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await mk.evaluate(() => new Promise(requestAnimationFrame));
    await mk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    const reopenedMasked = Buffer.from(await mk.locator("#canvas").evaluate(el =>
      Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data)));
    assert.deepEqual(reopenedMasked, expectedMasked);

    dlEv = mk.waitForEvent("download");
    await mk.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/masked.jxl`);
    assert.deepEqual(Buffer.from(decode_jxl(await readFile(`${output}/masked.jxl`)).into_pixels()), expectedMasked);
    assert.deepEqual(mkErrors, []);
    await mk.close();
  }

  // ── Phase 2: ZenSR Neural JPEG Restoration (dejpeg_rt24g in Browser WASM SIMD + Non-Destructive Round-Trip) ──
  {
    const sr = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const srErrors = [];
    sr.on("pageerror", error => srErrors.push(error.message));
    await sr.goto(baseUrl);
    await sr.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sr.getByRole("button", { name: "Browse gallery", exact: true }).click();
    await sr.locator('[data-gallery-id="lowq-jpeg-demo"] button[data-action="add"]').click();
    await sr.locator("#status").filter({ hasText: "Image opened" }).waitFor();

    const readSrCanvas = () => sr.locator("#canvas").evaluate(el =>
      Array.from(el.getContext("2d").getImageData(0, 0, el.width, el.height).data));
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    const beforeSr = Buffer.from(await readSrCanvas());

    await sr.getByRole("button", { name: "ZenSR", exact: true }).click();
    await sr.locator("#zensr-enabled").check();
    await sr.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    await sr.locator("#zensr-telemetry").filter({ hasText: /ZenSR active \(dejpeg_rt24g, 1×\).*ms/ }).waitFor();
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    const afterSr = Buffer.from(await readSrCanvas());
    assert.notDeepEqual(afterSr, beforeSr, "ZenSR dejpeg_rt24g must modify low-Q JPEG pixels");
    await sr.screenshot({ path: `${output}/zensr.png` });
    await writeFile(`${output}/zensr.rgba`, afterSr);

    let dlEv = sr.waitForEvent("download");
    await sr.getByRole("button", { name: "Save project", exact: true }).click();
    await (await dlEv).saveAs(`${output}/zensr.project.jxl`);
    const zensrProj = await readFile(`${output}/zensr.project.jxl`);
    const zParsed = readProject(zensrProj);
    assert.equal(zParsed.manifest.layers[0].zensr.enabled, true);
    assert.equal(zParsed.manifest.layers[0].zensr.model, "dejpeg_rt24g");
    // Original JPEG bytes preserved untouched in znas
    const origSrc = zParsed.assets.get(zParsed.manifest.layers[0].id).source;
    assert.equal(origSrc[0], 0xff);
    assert.equal(origSrc[1], 0xd8);
    assert.deepEqual(Buffer.from(decode_jxl(zensrProj).into_pixels()), afterSr);

    await sr.reload();
    await sr.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sr.locator("#open-file").setInputFiles({ name: "zensr.project.jxl", mimeType: "image/jxl", buffer: zensrProj });
    await sr.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    await sr.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    assertBytesEqual(Buffer.from(await readSrCanvas()), afterSr, "reopened zensr 1x");

    dlEv = sr.waitForEvent("download");
    await sr.getByRole("button", { name: "Export JXL", exact: true }).click();
    await (await dlEv).saveAs(`${output}/zensr.jxl`);
    assertBytesEqual(Buffer.from(decode_jxl(await readFile(`${output}/zensr.jxl`)).into_pixels()), afterSr, "exported zensr 1x");
    // ── Phase 2B: ZenSR 2× Neural Upscaling (rtc_distill_2x → true 2× canvas expansion + non-destructive .project.jxl round-trip) ──
    await sr.getByRole("button", { name: "ZenSR", exact: true }).click();
    await sr.locator("#zensr-quick-2x-fast").click();
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    await sr.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    await sr.locator("#zensr-telemetry").filter({ hasText: /ZenSR active \(rtc_distill_2x, 2×.*→ 768×512\).*ms/ }).waitFor();
    assert.equal(await sr.locator("#dimensions").textContent(), "768 × 512 px",
      "ZenSR 2× upscale must expand canvas dimensions from 384×256 to 768×512");
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    const afterSr2x = Buffer.from(await readSrCanvas());
    assert.equal(afterSr2x.byteLength, 768 * 512 * 4);
    await sr.screenshot({ path: `${output}/zensr-2x.png` });
    await writeFile(`${output}/zensr-2x.rgba`, afterSr2x);

    dlEv = sr.waitForEvent("download");
    await sr.getByRole("button", { name: "Save project", exact: true }).click();
    await (await dlEv).saveAs(`${output}/zensr-2x.project.jxl`);
    const zensr2xProj = await readFile(`${output}/zensr-2x.project.jxl`);
    const z2Parsed = readProject(zensr2xProj);
    assert.equal(z2Parsed.manifest.width, 768);
    assert.equal(z2Parsed.manifest.height, 512);
    assert.equal(z2Parsed.manifest.layers[0].width, 384, "original layer asset width stays untouched");
    assert.equal(z2Parsed.manifest.layers[0].zensr.model, "rtc_distill_2x");
    assert.equal(z2Parsed.manifest.layers[0].zensr.scale, 2);
    const dec2x = decode_jxl(zensr2xProj);
    assert.deepEqual([dec2x.width(), dec2x.height()], [768, 512]);
    assertBytesEqual(Buffer.from(dec2x.into_pixels()), afterSr2x, "saved zensr-2x presentation");

    // Reopen 2× upscaled project and verify bit-for-bit restoration
    await sr.reload();
    await sr.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sr.locator("#open-file").setInputFiles({ name: "zensr-2x.project.jxl", mimeType: "image/jxl", buffer: zensr2xProj });
    await sr.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    await sr.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    assert.equal(await sr.locator("#dimensions").textContent(), "768 × 512 px");
    await sr.evaluate(() => new Promise(requestAnimationFrame));
    assertBytesEqual(Buffer.from(await readSrCanvas()), afterSr2x, "reopened zensr-2x canvas");

    // Verify Browser Storage (IndexedDB Versions & Copies) + In-App Console Log!
    await sr.locator("#vault-tab").click();
    await sr.locator("#vault-label").fill("Upscaled 2x Snapshot");
    await sr.locator("#vault-save-snapshot").click();
    await sr.locator("#status").filter({ hasText: /Version "Upscaled 2x Snapshot" saved/ }).waitFor();
    await sr.locator("#vault-label").fill("Upscaled Copy");
    await sr.locator("#vault-save-copy").click();
    await sr.locator("#status").filter({ hasText: /Saved copy "Upscaled Copy"/ }).waitFor();

    // Mutate exposure so we can prove Restore brings back the exact snapshot state
    await sr.locator("#properties-tab").click();
    await sr.locator("#adjust-exposure").fill("1.5");
    await sr.locator("#adjust-exposure").dispatchEvent("input");
    await sr.locator("#adjust-exposure").dispatchEvent("change");
    assert.equal(Number(await sr.locator("#adjust-exposure").inputValue()), 1.5);

    // Reload page completely and verify IndexedDB persists copies & versions across reload
    await sr.reload();
    await sr.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await sr.locator("#empty-resume").waitFor({ state: "visible" });
    await sr.locator("#vault-tab").click();
    await sr.locator("#vault-docs-list .vault-card").filter({ hasText: "Upscaled Copy" }).waitFor({ state: "visible" });
    await sr.locator("#vault-docs-list .vault-card").filter({ hasText: "Upscaled Copy" }).getByRole("button", { name: "Open", exact: true }).click();
    await sr.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await sr.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    assert.equal(Number(await sr.locator("#adjust-exposure").inputValue()), 0, "Restored copy must have original exposure 0");
    assert.equal(await sr.locator("#dimensions").textContent(), "768 × 512 px");

    // Verify In-App Console Log captures codec & storage entries and flags errors
    await sr.locator("#log-toggle").click();
    await sr.locator("#log-panel").waitFor({ state: "visible" });
    const logCount = await sr.locator("#log-list .log-entry").count();
    assert.ok(logCount >= 3, `expected console log entries, got ${logCount}`);
    await sr.locator("#open-file").setInputFiles({ name: "broken.jxl", mimeType: "image/jxl", buffer: Buffer.from([255, 10, 0, 0, 0, 0]) });
    await sr.locator("#log-toggle[data-has-error]").waitFor();
    await sr.locator('#log-filters [data-log-filter="errors"]').click();
    assert.ok(await sr.locator("#log-list .log-error").count() >= 1, "expected error entry in console log");
    await sr.screenshot({ path: `${output}/vault-log.png` });

    // Verify JXL Native Document Structure Inspector (Native vs Prerendered vs Editor Data + inline property editor + Plain JXL filter)
    await sr.locator("#mode-structure").click();
    await sr.locator("#structure-panel").waitFor({ state: "visible" });
    assert.ok(await sr.locator("#struct-tree .prov-native").count() >= 1, "expected NATIVE JXL nodes in structure tree");
    assert.ok(await sr.locator("#struct-tree .prov-prerendered").count() >= 1, "expected PRERENDERED nodes in structure tree");
    assert.ok(await sr.locator("#struct-tree .prov-editor").count() >= 1, "expected EDITOR DATA nodes in structure tree");
    // Plain JXL filter hides editor-only boxes
    await sr.locator('#struct-filters [data-struct-filter="plain"]').click();
    assert.equal(await sr.locator("#struct-tree .prov-editor").count(), 0, "Plain JXL filter must hide editor-only boxes");
    await sr.locator('#struct-filters [data-struct-filter="all"]').click();
    // Directly edit layer property via Structure Inspector property editor
    await sr.locator('#struct-tree [data-node-id^="layer:"]').first().click();
    const structRotInput = sr.locator("#struct-props input[type='number']").nth(3);
    await structRotInput.fill("15");
    await structRotInput.dispatchEvent("change");
    assert.equal(Number(await sr.locator("#adjust-angle").inputValue()), 15, "Structure inspector direct edit must update live layer rotation");
    // Inspect & validate live zned manifest JSON box
    await sr.locator('#struct-tree [data-node-id="box:zned"]').click();
    const manifestJson = JSON.parse(await sr.locator("#struct-manifest-json").inputValue());
    assert.equal(manifestJson.width, 768);
    assert.equal(manifestJson.layers[0].angle, 15);
    await sr.screenshot({ path: `${output}/structure-inspector.png` });
    await sr.locator("#mode-studio").click();

    // Verify Canva contextual bar Duplicate layer button
    await sr.locator("#ctx-dup").click();
    assert.equal(await sr.locator(".layer").count(), 2, "Contextual Duplicate button must add a cloned layer");

    // Clean up browser storage after test
    await sr.locator("#vault-tab").click();
    sr.once("dialog", d => d.accept());
    await sr.locator("#vault-clear-all").click();

    assert.deepEqual(srErrors, []);
    await sr.close();
  }

  // ── Touch & Pointer Direct Resize / Rotate (1-layer handles, real CDP touch drag, 2-finger pinch+twist, quick-tap buttons) ──
  {
    const touchCtx = await browser.newContext({
      viewport: { width: 390, height: 844 },
      hasTouch: true,
      isMobile: true,
    });
    const tp = await touchCtx.newPage();
    const tpErrors = [];
    tp.on("pageerror", error => tpErrors.push(error.message));
    await tp.goto(baseUrl);
    await tp.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await tp.getByRole("button", { name: "Try a sample" }).click();
    await tp.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    await tp.locator("#outline .handle-se").waitFor({ state: "visible" });
    await tp.locator("#outline .handle-rotate").waitFor({ state: "visible" });
    assert.equal(await tp.evaluate(() => document.documentElement.scrollWidth), 390);

    const cdp = await touchCtx.newCDPSession(tp);
    const sendTouch = (type, points) => cdp.send("Input.dispatchTouchEvent", {
      type,
      touchPoints: points.map(([id, x, y]) => ({ id, x: Math.round(x), y: Math.round(y), radiusX: 12, radiusY: 12, force: 1 })),
    });

    // 1. Real single-finger touch drag on SE corner handle to shrink 1-layer photo
    const seBox = await tp.locator("#outline .handle-se").boundingBox();
    const seX = seBox.x + seBox.width / 2;
    const seY = seBox.y + seBox.height / 2;
    await sendTouch("touchStart", [[1, seX, seY]]);
    for (let i = 1; i <= 6; i++) {
      await sendTouch("touchMove", [[1, seX - i * 14, seY - i * 9]]);
    }
    await sendTouch("touchEnd", []);
    const afterTouchScale = Number(await tp.locator("#adjust-scale").inputValue());
    assert.ok(afterTouchScale < 0.9, `touch drag on SE handle must shrink layer, got scale=${afterTouchScale}`);
    assert.equal(Number(await tp.locator("#position-x").inputValue()), 0, "SE resize must keep NW corner pinned at x=0");
    assert.equal(Number(await tp.locator("#position-y").inputValue()), 0, "SE resize must keep NW corner pinned at y=0");

    // 2. Real single-finger touch drag on golden rotate handle to rotate layer around its center
    const rotBox = await tp.locator("#outline .handle-rotate").boundingBox();
    const rX = rotBox.x + rotBox.width / 2;
    const rY = rotBox.y + rotBox.height / 2;
    await sendTouch("touchStart", [[2, rX, rY]]);
    for (let i = 1; i <= 6; i++) {
      await sendTouch("touchMove", [[2, rX + i * 10, rY + i * 4]]);
    }
    await sendTouch("touchEnd", []);
    const afterTouchAngle = Number(await tp.locator("#adjust-angle").inputValue());
    assert.ok(afterTouchAngle > 10, `touch drag on rotate handle must rotate clockwise, got angle=${afterTouchAngle}`);
    assert.equal(await tp.evaluate(() => document.documentElement.scrollWidth), 390,
      "rotated outline on 390px mobile must not expand document scrollWidth");

    // 3. Real 2-finger pinch + twist directly on #canvas
    const cBox = await tp.locator("#canvas").boundingBox();
    const cx = cBox.x + cBox.width / 2;
    const cy = cBox.y + cBox.height / 2;
    await sendTouch("touchStart", [[11, cx - 40, cy]]);
    await sendTouch("touchStart", [[11, cx - 40, cy], [12, cx + 40, cy]]);
    for (let i = 1; i <= 5; i++) {
      const r = 40 + i * 6; // expand from 80px -> 140px span (+75% scale)
      const rad = (-i * 5) * Math.PI / 180; // twist -25° counter-clockwise
      await sendTouch("touchMove", [
        [11, cx - r * Math.cos(rad), cy - r * Math.sin(rad)],
        [12, cx + r * Math.cos(rad), cy + r * Math.sin(rad)],
      ]);
    }
    await sendTouch("touchEnd", [[12, cx + 70, cy]]);
    await sendTouch("touchEnd", []);
    const afterPinchScale = Number(await tp.locator("#adjust-scale").inputValue());
    const afterPinchAngle = Number(await tp.locator("#adjust-angle").inputValue());
    assert.ok(afterPinchScale > afterTouchScale + 0.15,
      `2-finger pinch out must increase scale (${afterTouchScale} -> ${afterPinchScale})`);
    assert.ok(afterPinchAngle < afterTouchAngle - 10,
      `2-finger twist CCW must decrease angle (${afterTouchAngle} -> ${afterPinchAngle})`);

    // 4. Quick-tap transform buttons (+90°, -90°, 100%)
    await tp.locator("#scale-reset").click();
    assert.equal(Number(await tp.locator("#adjust-scale").inputValue()), 1);
    assert.equal(Number(await tp.locator("#adjust-angle").inputValue()), 0);
    await tp.locator("#rotate-cw").click();
    assert.equal(Number(await tp.locator("#adjust-angle").inputValue()), 90);
    await tp.locator("#rotate-ccw").click();
    assert.equal(Number(await tp.locator("#adjust-angle").inputValue()), 0);

    // 5. Desktop mouse rotate + NW corner resize verification + screenshot
    const desk = await browser.newPage({ viewport: { width: 1400, height: 940 } });
    const deskErrors = [];
    desk.on("pageerror", error => deskErrors.push(error.message));
    await desk.goto(baseUrl);
    await desk.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await desk.getByRole("button", { name: "Try a sample" }).click();
    await desk.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    const nwBox = await desk.locator("#outline .handle-nw").boundingBox();
    await desk.mouse.move(nwBox.x + nwBox.width / 2, nwBox.y + nwBox.height / 2);
    await desk.mouse.down();
    await desk.mouse.move(nwBox.x + nwBox.width / 2 + 160, nwBox.y + nwBox.height / 2 + 105, { steps: 6 });
    await desk.mouse.up();
    const nwScale = Number(await desk.locator("#adjust-scale").inputValue());
    assert.ok(nwScale < 0.88, `NW mouse drag must shrink layer, got ${nwScale}`);
    const dRotBox = await desk.locator("#outline .handle-rotate").boundingBox();
    await desk.mouse.move(dRotBox.x + dRotBox.width / 2, dRotBox.y + dRotBox.height / 2);
    await desk.mouse.down();
    await desk.mouse.move(dRotBox.x + dRotBox.width / 2 + 95, dRotBox.y + dRotBox.height / 2 + 30, { steps: 6 });
    await desk.mouse.up();
    const dAngle = Number(await desk.locator("#adjust-angle").inputValue());
    assert.ok(dAngle > 10, `rotate handle mouse drag must rotate layer, got ${dAngle}`);
    await desk.evaluate(() => new Promise(requestAnimationFrame));
    await desk.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"));
    await desk.screenshot({ path: `${output}/photo-transform.png` });
    assert.deepEqual(deskErrors, []);
    await desk.close();

    assert.deepEqual(tpErrors, []);
    await touchCtx.close();
  }

  // ── Phase 3: Multi-Decoder Check & Compatibility Lab (zenjxl-decoder, jxl-rs, jxl-oxide, libjxl) ──
  {
    const dc = await browser.newPage({ acceptDownloads: true, viewport: { width: 1400, height: 940 } });
    const dcErrors = [];
    dc.on("pageerror", error => dcErrors.push(error.message));
    await dc.goto(baseUrl);
    await dc.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await dc.locator("#open-file").setInputFiles({ name: "gradient.jxl", mimeType: "image/jxl", buffer: source });
    await dc.locator("#status").filter({ hasText: "Image opened" }).waitFor();

    // 1. Click topbar "Check decoding" button -> loads all 4 WASM decoders & verifies bit-exact match
    await dc.locator("#check-decoding").click();
    await dc.locator('#dc-latest [data-decoder="libjxl"]').waitFor({ timeout: 30000 });
    for (const id of ["zenjxl-decoder", "jxl-rs", "jxl-oxide", "libjxl"]) {
      const cardText = await dc.locator(`#dc-latest [data-decoder="${id}"]`).textContent();
      assert.ok(cardText.includes("OK"), `${id} must decode gradient.jxl cleanly, got: ${cardText}`);
    }
    const verdictText = await dc.locator("#dc-latest .dc-verdict").textContent();
    assert.ok(verdictText.includes("ALL EXACT"), `All 4 decoders must agree bit-for-bit on lossless gradient.jxl, got: ${verdictText}`);
    assert.equal(await dc.locator("#dc-diff-canvas").isVisible(), true, "Diff canvas must render for passing decoders");

    // 2. Run 10-mode Compatibility Sweep loop across all 4 decoders
    await dc.locator("#dc-sweep-btn").click();
    await dc.locator("#status").filter({ hasText: /Sweep complete: 10 cases/ }).waitFor({ timeout: 60000 });
    const surfaceRows = await dc.locator("#dc-surface tbody tr").count();
    assert.ok(surfaceRows >= 8, `Compatibility surface must list >= 8 JXL features, got ${surfaceRows}`);
    const savedCount = await dc.locator("#dc-history-list .dc-run-item").count();
    assert.ok(savedCount >= 11, `Expected >= 11 saved runs in IndexedDB (1 + 10 sweep), got ${savedCount}`);

    // Log sweep summary so any real cross-decoder findings surface in test output!
    const sweepPayload = await dc.evaluate(() => window.jxlStudioDecodeLab.buildExportPayload({ includeBytes: false }));
    console.log("Decode Lab 10-mode sweep summary:", JSON.stringify(sweepPayload.summary));
    if (sweepPayload.suspectedBugs.length) {
      console.log("Decode Lab findings:", JSON.stringify(sweepPayload.suspectedBugs, null, 2));
    }
    await dc.screenshot({ path: `${output}/decode-lab.png` });

    // 3. Verify IndexedDB persistence across reload
    await dc.reload();
    await dc.getByText("Relaxed SIMD", { exact: true }).waitFor();
    await dc.locator("#decode-tab").click();
    await dc.waitForFunction(() => document.querySelectorAll("#dc-history-list .dc-run-item").length >= 11);

    // 4. Verify Export JSON, Export NDJSON, and Export Bug MD downloads
    let dlEv = dc.waitForEvent("download");
    await dc.locator("#dc-export-json").click();
    const jsonDl = await dlEv;
    assert.match(jsonDl.suggestedFilename(), /^jxl-decode-lab-.*\.json$/);
    const jsonParsed = JSON.parse(await readFile(await jsonDl.path(), "utf8"));
    assert.equal(jsonParsed.schemaVersion, 1);
    assert.ok(jsonParsed.runs.length >= 11);
    assert.ok(Array.isArray(jsonParsed.compatibilitySurface) && jsonParsed.compatibilitySurface.length >= 8);

    dlEv = dc.waitForEvent("download");
    await dc.locator("#dc-export-ndjson").click();
    const ndDl = await dlEv;
    const ndLines = (await readFile(await ndDl.path(), "utf8")).trim().split("\n");
    assert.ok(ndLines.length >= 11);
    assert.ok(JSON.parse(ndLines[0]).results.length === 4);

    dlEv = dc.waitForEvent("download");
    await dc.locator("#dc-export-md").click();
    const mdDl = await dlEv;
    const mdText = await readFile(await mdDl.path(), "utf8");
    assert.ok(mdText.includes("| Feature | zenjxl-decoder | jxl-rs | jxl-oxide | libjxl |"));

    // 5. Verify programmatic automation API (window.jxlStudioDecodeLab.checkBytes) catches a broken bitstream cleanly
    const brokenRec = await dc.evaluate(async () => {
      const broken = new Uint8Array([0x00, 0x00, 0x00, 0x0c, 0x4a, 0x58, 0x4c, 0x20, 0x0d, 0x0a, 0x87, 0x0a, 0xff, 0x0a, 0x00]);
      return window.jxlStudioDecodeLab.checkBytes(broken, { label: "broken-repro.jxl", sourceKind: "loop" });
    });
    assert.equal(brokenRec.verdict, "all-failed");
    assert.equal(brokenRec.results.filter(r => !r.ok).length, 4, "All 4 decoders must reject broken bitstream without trapping");

    assert.deepEqual(dcErrors, []);
    await dc.close();
  }

  if (process.env.EDITOR_JPEG_CARRIER) {
    const resources = await browser.newPage({ acceptDownloads: true });
    await resources.goto(baseUrl);
    const carrier = await readFile(process.env.EDITOR_JPEG_CARRIER);
    await resources.locator("#open-file").setInputFiles({
      name: "reconstruction.jxl", mimeType: "image/jxl", buffer: carrier,
    });
    await resources.getByText("Image opened", { exact: true }).waitFor();
    await resources.getByRole("button", { name: "Native JXL", exact: true }).click();
    await resources.getByText("Image header fields", { exact: true }).click();
    const iccDownload = resources.waitForEvent("download");
    await resources.getByRole("button", { name: "Download embedded ICC" }).click();
    const icc = await iccDownload;
    assert.equal(icc.suggestedFilename(), "reconstruction.icc");
    assert.deepEqual(await readFile(await icc.path()), await readFile(process.env.EDITOR_ICC_EXPECTED));
    await resources.getByText("Container boxes", { exact: true }).click();
    const jpgDownload = resources.waitForEvent("download");
    await resources.getByRole("button", { name: "Reconstruct original JPEG" }).click();
    const jpg = await jpgDownload;
    assert.equal(jpg.suggestedFilename(), "reconstruction.jpg");
    const original = await readFile(new URL("../dist/sample.jpg", import.meta.url));
    assert.deepEqual(await readFile(await jpg.path()), original);
    console.log("Embedded ICC and reconstructed JPEG downloaded byte for byte.");
    await resources.close();
  }

  // Serve a worker whose capability probe reports the unsupported branch.
  // This avoids dependence on obsolete Chromium V8 switches or interception
  // of fetches that originate inside a dedicated worker.
  const names = { ".html": "text/html", ".mjs": "text/javascript", ".js": "text/javascript",
    ".css": "text/css", ".wasm": "application/wasm", ".raw": "application/octet-stream", ".jpg": "image/jpeg" };
  const worker = await readFile(new URL("../dist/codec-worker.mjs", import.meta.url), "utf8");
  assert.ok(worker.includes("WebAssembly.validate(relaxedProbe)"));
  const fallbackServer = createServer(async (req, res) => {
    try {
      const pathname = req.url === "/" ? "/index.html" : req.url;
      if (!/^\/[a-zA-Z0-9_./-]+\.[a-z]+$/.test(pathname) || pathname.includes("..") || !names[extname(pathname)]) {
        res.writeHead(404).end();
        return;
      }
      const body = pathname === "/codec-worker.mjs"
        ? worker.replace("WebAssembly.validate(relaxedProbe)", "false")
        : await readFile(new URL(`../dist${pathname}`, import.meta.url));
      res.writeHead(200, { "Content-Type": names[extname(pathname)] }).end(body);
    } catch {
      res.writeHead(404).end();
    }
  });
  fallbackServer.listen(3762, "127.0.0.1");
  await once(fallbackServer, "listening");
  try {
    const baseline = await browser.newPage({ acceptDownloads: true });
    await baseline.goto("http://127.0.0.1:3762/");
    await baseline.getByText("Wasm compatibility", { exact: true }).waitFor();
    await baseline.locator("#open-file").setInputFiles({ name: "fallback.jxl", mimeType: "image/jxl", buffer: source });
    await baseline.locator("#status").filter({ hasText: "Image opened" }).waitFor();
    await baseline.locator("#adjust-exposure").press("ArrowRight");
    const download = baseline.waitForEvent("download");
    await baseline.getByRole("button", { name: "Save project" }).click();
    const fallbackPath = `${output}/fallback.project.jxl`;
    const file = await download;
    assert.equal(file.suggestedFilename(), "fallback.project.jxl");
    await file.saveAs(fallbackPath);
    const fallbackFile = await readFile(fallbackPath);
    assert.equal(readProject(fallbackFile).manifest.layers[0].exposure, 0.01);
    await baseline.reload();
    await baseline.locator("#open-file").setInputFiles({ name: "fallback.project.jxl", mimeType: "image/jxl", buffer: fallbackFile });
    await baseline.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await baseline.getByRole("button", { name: "Native JXL", exact: true }).click();
    await baseline.getByText("1 native JPEG XL frame", { exact: true }).waitFor();
    await baseline.getByRole("button", { name: "Add frame as layer", exact: true }).waitFor();
    // Also exercise ZenSR baseline WASM compatibility build (-simd128,-relaxed-simd)
    await baseline.getByRole("button", { name: "ZenSR", exact: true }).click();
    await baseline.locator("#zensr-enabled").check();
    await baseline.waitForFunction(() => !document.getElementById("canvas").hasAttribute("data-filter-pending"), { timeout: 30000 });
    await baseline.locator("#zensr-telemetry").filter({ hasText: /ZenSR active \(dejpeg_rt24g, 1×\).*ms/ }).waitFor();
    await baseline.close();
  } finally {
    fallbackServer.close();
  }
  console.log(`Chromium ${browser.version()}: relaxed SIMD and compatibility paths passed. Layer edits, source vectors, SVG raster, save, reload and flat export verified.`);
} finally {
  await browser.close();
}
