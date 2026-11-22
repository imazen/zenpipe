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
  await photo.screenshot({ path: `${output}/photo.png` });
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
    ".css": "text/css", ".wasm": "application/wasm" };
  const worker = await readFile(new URL("../dist/codec-worker.mjs", import.meta.url), "utf8");
  assert.ok(worker.includes("WebAssembly.validate(relaxedProbe)"));
  const fallbackServer = createServer(async (req, res) => {
    try {
      const pathname = req.url === "/" ? "/index.html" : req.url;
      if (!/^\/[a-zA-Z0-9_/-]+\.[a-z]+$/.test(pathname) || !names[extname(pathname)]) {
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
    await baseline.close();
  } finally {
    fallbackServer.close();
  }
  console.log(`Chromium ${browser.version()}: relaxed SIMD and compatibility paths passed. Layer edits, source vectors, SVG raster, save, reload and flat export verified.`);
} finally {
  await browser.close();
}
