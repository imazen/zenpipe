import test from "node:test";
import assert from "node:assert/strict";
import { box, jxlBoxes, opaqueBoxes, readProject, writeProject } from "../web/project.mjs";

const tex = new TextEncoder();
const bare = Uint8Array.of(255, 10, 1, 2, 3);
const id = "016f1c0a-23c7-4478-b25c-acde0000f888";
const layer = { id, name: "Overlay & text", mime: "image/svg+xml", width: 5, height: 3,
  x: 2, y: -1, scale: 1.25, angle: -43, visible: true, opacity: 0.75,
  exposure: 0.15, contrast: 1, saturation: 0.82 };
const manifest = { version: 1, width: 13, height: 17, originalId: null, layers: [layer] };
const source = tex.encode("\uFEFF<svg>\r\n  <!-- untouched -->\r\n</svg>");
const svgEdit = tex.encode("<svg><!-- replacement --></svg>");
const assets = new Map([[id, { source, svgEdit, rendition: bare }]]);
const bytes = async blob => new Uint8Array(await blob.arrayBuffer());

test("round-trips source bytes, vector edits, pixels, transforms and opaque boxes", async () => {
  const foreign = { type: "abCD", data: Uint8Array.of(9, 8, 7), extendsToEnd: true };
  const output = await bytes(writeProject(bare, manifest, assets, [foreign]));
  const reloaded = readProject(output);
  assert.deepEqual(reloaded.manifest, manifest);
  assert.deepEqual(reloaded.assets.get(id), assets.get(id));
  assert.deepEqual(reloaded.preserved[0].data, foreign.data);
  assert.deepEqual(jxlBoxes(output).find(entry => entry.type === "jxlc").data, bare);
  assert.equal(readProject(bare), null);
});

test("reads extended-size and to-EOF boxes and rejects truncation", async () => {
  const project = await bytes(writeProject(bare, manifest, assets));
  const long = Uint8Array.of(0, 0, 0, 1, 115, 116, 101, 115, 0, 0, 0, 0, 0, 0, 0, 17, 6);
  const toEnd = Uint8Array.of(0, 0, 0, 0, 120, 88, 120, 88, 2, 1);
  const extended = await bytes(new Blob([project, long, toEnd]));
  assert.deepEqual(opaqueBoxes(extended).map(entry => [entry.type, ...entry.data]), [
    ["stes", 6], ["xXxX", 2, 1],
  ]);
  assert.throws(() => readProject(extended.subarray(0, project.length + 14)), /truncated/i);
});

test("rejects duplicate and missing asset boxes and unknown manifest versions", async () => {
  const saved = await bytes(writeProject(bare, manifest, assets));
  const [duplicate] = jxlBoxes(saved).filter(entry => entry.type === "znas");
  assert.throws(() => writeProject(bare, { ...manifest, version: 2 }, assets), /version/i);
  assert.throws(() => writeProject(bare, manifest, new Map()), /missing/i);
  const bad = await bytes(new Blob([saved, duplicate.raw]));
  assert.throws(() => readProject(bad), /asset/i);
  const extraManifest = await bytes(new Blob([saved, ...box("zned", tex.encode("{}"))]));
  assert.throws(() => readProject(extraManifest), /manifest/i);
});
