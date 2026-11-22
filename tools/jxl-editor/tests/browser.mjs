import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { once } from "node:events";
import { createServer } from "node:http";
import { extname } from "node:path";
import { chromium } from "playwright";
import init, { decode_jxl, encode_jxl } from "../dist/wasm/relaxed/jxl_editor_codecs.js";
import { box, jxlBoxes, opaqueBoxes, readProject } from "../web/project.mjs";

const baseUrl = process.env.EDITOR_URL ?? "http://127.0.0.1:3761";
const output = process.env.EDITOR_OUTPUT ?? "test-results";
await mkdir(output, { recursive: true });
await init({ module_or_path: await readFile(new URL("../dist/wasm/relaxed/jxl_editor_codecs_bg.wasm", import.meta.url)) });

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
  const expected = Buffer.from(await preview());
  const saved = await capture("Save project", `${output}/gradient.project.jxl`);
  await waitStatus("Project saved");
  await writeFile(`${output}/gradient.rgba`, expected);
  const project = readProject(saved);
  assert.deepEqual(project.manifest.layers.map(layer => layer.name), ["gradient.jxl", "vector.svg", "overlay.jxl"]);
  assert.equal(project.manifest.layers[2].visible, false);
  assert.equal(project.manifest.layers[1].opacity, 0.99);
  assert.equal(project.manifest.layers[1].exposure, 0.01);
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
    await (await download).saveAs(fallbackPath);
    const fallbackFile = await readFile(fallbackPath);
    assert.equal(readProject(fallbackFile).manifest.layers[0].exposure, 0.01);
    await baseline.reload();
    await baseline.locator("#open-file").setInputFiles({ name: "fallback.project.jxl", mimeType: "image/jxl", buffer: fallbackFile });
    await baseline.locator("#status").filter({ hasText: "Project reopened" }).waitFor();
    await baseline.close();
  } finally {
    fallbackServer.close();
  }
  console.log(`Chromium ${browser.version()}: relaxed SIMD and compatibility paths passed. Layer edits, source vectors, SVG raster, save, reload and flat export verified.`);
} finally {
  await browser.close();
}
