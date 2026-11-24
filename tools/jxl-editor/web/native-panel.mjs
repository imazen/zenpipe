import { codec } from "./codec.mjs";
import { jxlBoxes } from "./project.mjs";

const $ = id => document.getElementById(id);
const littleEndian = new Uint8Array(Uint32Array.of(1).buffer)[0] === 1;

function floatBytes(samples) {
  const bytes = new Uint8Array(samples.buffer, samples.byteOffset, samples.byteLength);
  if (littleEndian) return bytes;
  const result = new Uint8Array(bytes.length);
  for (let i = 0; i < bytes.length; i += 4) {
    result.set([bytes[i + 3], bytes[i + 2], bytes[i + 1], bytes[i]], i);
  }
  return result;
}

function options(select, names) {
  select.replaceChildren();
  names.forEach((name, i) => select.add(new Option(name, String(i))));
  select.disabled = !names.length;
}

export function makeNativeInspector({
  run, download, canvasFrom, addNativeLayer, importNativeTimeline, loadNativeCmyk, report, status,
}) {
  let source;
  let sourceName;
  let summary;
  let frame;
  let boxes = [];
  let channels = [];
  let request = 0;
  let busy = false;

  function composable() {
    return frame && ["Replace", "Blend"].includes(frame.info.colorBlend.mode);
  }

  function updateControls() {
    $("native-frame").disabled = busy || !summary?.frames.length;
    $("native-plane").disabled = busy || !frame;
    $("native-add").disabled = busy || !frame?.preview.length || !composable();
    $("native-plane-download").disabled = busy || !frame?.planes.length;
    $("native-box").disabled = busy || !boxes.length;
    $("native-box-download").disabled = busy || !boxes.length;
    $("native-icc").disabled = busy;
    $("native-jpeg").disabled = busy;
    $("native-import-timeline").disabled = busy || !summary;
    $("native-load-cmyk").disabled = busy || !summary?.profile?.cmyk;
  }

  function renderFrame() {
    const preview = $("native-preview");
    preview.hidden = true;
    updateControls();
    $("native-frame-details").hidden = !frame;
    if (!frame) return;
    const channel = channels[Number($("native-plane").value)];
    const { info, planes } = frame;
    const frameIdx = Number($("native-frame").value);
    const mergedInfo = summary?.frames?.[frameIdx]?.coding
      ? { ...info, coding: summary.frames[frameIdx].coding }
      : info;
    $("native-frame-json").textContent = JSON.stringify(mergedInfo, null, 2);
    const [x, y] = info.origin;
    const [w, h] = info.size;
    $("native-info").textContent = `${w} × ${h} at (${x}, ${y}) · ${info.colorBlend.mode}`;
    if (!composable()) $("native-info").textContent += " · reference-dependent samples";
    if (!channel.raw && !frame.preview.length) {
      $("native-info").textContent += ` · ${frame.previewError}`;
      return;
    }
    let pixels = frame.preview;
    let [width, height] = info.displaySize;
    if (channel.raw) {
      const plane = planes[channel.plane];
      const { offset, stride } = channel;
      const length = plane.length / stride;
      let min = Infinity;
      let max = -Infinity;
      for (let i = offset; i < plane.length; i += stride) {
        if (!Number.isFinite(plane[i])) continue;
        min = Math.min(min, plane[i]);
        max = Math.max(max, plane[i]);
      }
      const low = Math.min(0, min);
      const high = Math.max(1, max);
      pixels = new Uint8ClampedArray(length * 4);
      for (let i = 0; i < length; i++) {
        const value = Math.round((plane[i * stride + offset] - low) * 255 / (high - low));
        pixels.set([value, value, value, 255], i * 4);
      }
      if (Number.isFinite(min) && Number.isFinite(max)) {
        $("native-info").textContent += ` · range ${min.toPrecision(4)} to ${max.toPrecision(4)}`;
      }
      [width, height] = info.size;
    }
    const image = canvasFrom({ width, height, pixels });
    preview.width = width;
    preview.height = height;
    const scale = Math.min(8, 250 / width, 160 / height);
    preview.style.width = `${Math.max(1, Math.round(width * scale))}px`;
    preview.style.imageRendering = scale > 1 ? "pixelated" : "auto";
    preview.getContext("2d").drawImage(image, 0, 0);
    preview.hidden = false;
  }

  async function loadFrame(index) {
    const token = ++request;
    const target = source;
    frame = null;
    renderFrame();
    const result = await codec("nativeFrame", { bytes: target.source, index, includePlanes: true });
    if (token !== request || target !== source) return;
    frame = result;
    const color = result.info.planeColorModel;
    const colorNames = color === "Cmyk" ? ["Cyan", "Magenta", "Yellow", "Black"]
      : color === "Grayscale" ? ["Gray"] : ["Red", "Green", "Blue"];
    channels = [{ name: "Color preview", plane: 0, raw: false },
      ...colorNames.map((name, offset) => ({ name, raw: true, plane: 0, offset, stride: colorNames.length })),
      ...summary.extraChannels.map((channel, i) => ({
        name: channel.name || channel.type, raw: true, plane: i + 1, offset: 0, stride: 1,
      })),
    ];
    options($("native-plane"), channels.map(channel => channel.name));
    renderFrame();
  }

  function updateBox() {
    const box = boxes[Number($("native-box").value)];
    $("native-box-status").textContent = box ? `${box.data.byteLength} payload bytes` : "Bare codestream";
  }

  $("native-frame").onchange = () => run("Reading local frame…", async () => {
    await loadFrame(Number($("native-frame").value));
    status("Local JPEG XL frame decoded");
  });
  $("native-plane").onchange = renderFrame;
  $("native-add").onclick = () => {
    if (busy || !source || !frame?.preview.length || !composable()) return;
    try {
      addNativeLayer(source, frame, Number($("native-frame").value));
    } catch (error) {
      report(error);
    }
  };
  $("native-import-timeline").onclick = () => {
    if (busy || !source || !importNativeTimeline) return;
    run("Importing animation frames to timeline…", () => importNativeTimeline(source, sourceName));
  };
  $("native-load-cmyk").onclick = () => {
    if (busy || !source || !summary?.profile?.cmyk || !loadNativeCmyk) return;
    run("Loading exact CMYK samples…", () => loadNativeCmyk(source, sourceName));
  };
  $("native-plane-download").onclick = () => {
    if (!frame) return;
    const index = channels[Number($("native-plane").value)].plane;
    const samples = frame.planes[index];
    const selected = Number($("native-frame").value) + 1;
    download(new Blob([floatBytes(samples)], { type: "application/octet-stream" }),
      `jxl-frame-${selected}-plane-${index}.f32le`);
  };
  async function saveResource(method, extension, type, message) {
    const target = source;
    const name = sourceName.replace(/\.jxl$/i, "");
    const bytes = await codec(method, { bytes: target.source });
    download(new Blob([bytes], { type }), `${name}.${extension}`);
    status(message);
  }
  $("native-icc").onclick = () => run("Reading embedded ICC…",
    () => saveResource("nativeIcc", "icc", "application/vnd.iccprofile", "Embedded ICC downloaded"));
  $("native-jpeg").onclick = () => run("Reconstructing the source JPEG…",
    () => saveResource("nativeJpeg", "jpg", "image/jpeg", "Original JPEG reconstructed"));

  $("native-box").onchange = updateBox;
  $("native-box-download").onclick = () => {
    const index = Number($("native-box").value);
    const box = boxes[index];
    if (box) download(new Blob([box.raw], { type: "application/octet-stream" }), `jxl-box-${index}.bin`);
  };

  return {
    setBusy(value) {
      busy = value;
      updateControls();
    },
    setSource(asset, name) {
      sourceName = name ?? "source";
      $("native-source-name").textContent = name ?? "";
      if (asset?.source === source?.source) return false;
      source = asset;
      summary = null;
      frame = null;
      channels = [];
      request++;
      try {
        boxes = asset ? jxlBoxes(asset.source) : [];
      } catch {
        boxes = [];
      }
      $("native-icc").hidden = true;
      $("native-import-timeline").hidden = true;
      $("native-load-cmyk").hidden = true;
      $("native-jpeg").hidden = !boxes.some(box => box.type === "jbrd" ||
        (box.type === "brob" && String.fromCharCode(...box.data.subarray(0, 4)) === "jbrd"));
      options($("native-frame"), []);
      options($("native-plane"), []);
      options($("native-box"), boxes.map(box => `${box.type} · ${box.data.byteLength} bytes`));
      $("native-image-details").hidden = true;
      $("native-info").textContent = "";
      renderFrame();
      updateBox();
      return true;
    },
    inspect() {
      if (!source || summary) return;
      const target = source;
      run("Reading native JPEG XL structure…", async () => {
        const result = await codec("nativeInfo", { bytes: target.source });
        if (target !== source) return;
        summary = result;
        $("native-image-json").textContent = JSON.stringify({ ...summary, frames: undefined }, null, 2);
        $("native-image-details").hidden = false;
        $("native-icc").hidden = !summary.profile.iccBytes;
        $("native-import-timeline").hidden = !(summary.animation || summary.frames.length > 1);
        $("native-load-cmyk").hidden = !summary.profile.cmyk;
        options($("native-frame"), summary.frames.map((layer, i) => {
          const [w, h] = layer.size;
          const enc = layer.coding?.encoding ? ` · ${layer.coding.encoding}` : "";
          return `Frame ${i + 1} · ${w} × ${h} · ${layer.colorBlend.mode}${enc}`;
        }));
        if (summary.frames.length) await loadFrame(0);
        const count = summary.frames.length;
        status(`${count} native JPEG XL ${count === 1 ? "frame" : "frames"}`);
      });
    },
  };
}
