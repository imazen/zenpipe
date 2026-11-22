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

test("round-trips timeline, manual splines, coding options and CMYK raw samples", async () => {
  const frameId1 = "016f1c0a-23c7-4478-b25c-acde0000f001";
  const frameId2 = "016f1c0a-23c7-4478-b25c-acde0000f002";
  const splineId = "016f1c0a-23c7-4478-b25c-acde0000f010";
  const richManifest = {
    ...manifest,
    timeline: {
      enabled: true,
      tpsNumerator: 1000,
      tpsDenominator: 1,
      loops: 2,
      frames: [
        {
          id: frameId1,
          name: "Frame 1",
          duration: 180,
          timecode: 100,
          blendMode: "auto",
          blendSource: null,
          saveAsReference: 1,
          referenceOnly: false,
          layerOverrides: { [id]: { visible: true, opacity: 0.75, x: 2, y: -1, scale: 1.25, angle: -43, exposure: 0.15, contrast: 1, saturation: 0.82 } },
        },
        {
          id: frameId2,
          name: "Frame 2",
          duration: 320,
          timecode: 200,
          blendMode: "replace",
          blendSource: null,
          saveAsReference: null,
          referenceOnly: false,
          layerOverrides: { [id]: { visible: true, opacity: 1, x: 6, y: 4, scale: 1, angle: 0, exposure: 0, contrast: 1, saturation: 1 } },
        },
      ],
    },
    splines: [
      {
        id: splineId,
        name: "Arc 1",
        colorHex: "#f5dc3c",
        sigma: 1.5,
        controlPoints: [[2, 3], [8, 12]],
        colorDct: [Array(32).fill(0), Array(32).fill(0), Array(32).fill(0)],
        sigmaDct: [1.06, ...Array(31).fill(0)],
        bakeIntoSource: true,
      },
    ],
    coding: { mode: "vardct", distance: 1.5, effort: 5, progressive: "quantized_ac" },
    cmyk: { enabled: true, bitDepth: 8, mode: "modular", distance: 1, effort: 3, iccName: "Test CMYK", origin: "source", channelOffsets: [0, 0, 0, 0] },
  };
  const fakeIcc = new Uint8Array(128);
  fakeIcc.set(tex.encode("CMYK"), 16);
  const cmykSamples = new Uint8Array(13 * 17 * 4).fill(200);
  const output = await bytes(writeProject(bare, richManifest, assets, [], { samples: cmykSamples, icc: fakeIcc }));
  const reloaded = readProject(output);
  assert.deepEqual(reloaded.manifest, richManifest);
  assert.deepEqual(reloaded.cmykData.samples, cmykSamples);
  assert.deepEqual(reloaded.cmykData.icc, fakeIcc);
  assert.equal(reloaded.preserved.length, 0);
});
