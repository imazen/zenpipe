// =====================================================================
// Deterministic generated RGBA fixture + model operations (zenpipe#113)
//
// The fixture is a closed-form function of (x, y, seed): no tables, no
// randomness, no assets. Any consumer — the lab page, a Node test, a
// Playwright spec — can recompute every expected pixel exactly.
//
// `seed` is the source generation: bumping it changes the pixels, which
// is how the shell demonstrates superseded-output rejection.
// =====================================================================

export const FIXTURE_WIDTH = 96;
export const FIXTURE_HEIGHT = 64;
export const FIXTURE_BYTES = FIXTURE_WIDTH * FIXTURE_HEIGHT * 4;

/** A generated frame: `{ width, height, data: Uint8ClampedArray }` RGBA8. */
export function generateFixture(seed = 0, width = FIXTURE_WIDTH, height = FIXTURE_HEIGHT) {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      data[i] = (x * 3 + y * 5 + seed * 17) % 256; // R: diagonal gradient
      data[i + 1] = (x * 7 + seed * 29) % 256; //     G: vertical stripes
      data[i + 2] = (y * 11 + seed * 41) % 256; //    B: horizontal stripes
      data[i + 3] = 255; //                          opaque
    }
  }
  return { width, height, data };
}

/** Expected value of one fixture pixel, for tests that don't regenerate. */
export function fixturePixel(x, y, seed = 0) {
  return [
    (x * 3 + y * 5 + seed * 17) % 256,
    (x * 7 + seed * 29) % 256,
    (y * 11 + seed * 41) % 256,
    255,
  ];
}

// ---------------------------------------------------------------------
// Model operations. These are deliberately trivial pixel transforms the
// deterministic backend applies to the generated fixture. They stand in
// for async jobs; they are NOT codec operations and qualify nothing.
// ---------------------------------------------------------------------

export const MODEL_OPS = Object.freeze({
  invert: Object.freeze({
    label: 'Invert RGB',
    apply({ width, height, data }) {
      const out = new Uint8ClampedArray(data.length);
      for (let i = 0; i < data.length; i += 4) {
        out[i] = 255 - data[i];
        out[i + 1] = 255 - data[i + 1];
        out[i + 2] = 255 - data[i + 2];
        out[i + 3] = data[i + 3];
      }
      return { width, height, data: out };
    },
  }),
  'swap-rb': Object.freeze({
    label: 'Swap R/B',
    apply({ width, height, data }) {
      const out = new Uint8ClampedArray(data.length);
      for (let i = 0; i < data.length; i += 4) {
        out[i] = data[i + 2];
        out[i + 1] = data[i + 1];
        out[i + 2] = data[i];
        out[i + 3] = data[i + 3];
      }
      return { width, height, data: out };
    },
  }),
  fade: Object.freeze({
    label: 'Half alpha',
    apply({ width, height, data }) {
      const out = new Uint8ClampedArray(data.length);
      for (let i = 0; i < data.length; i += 4) {
        out[i] = data[i];
        out[i + 1] = data[i + 1];
        out[i + 2] = data[i + 2];
        out[i + 3] = data[i + 3] >> 1;
      }
      return { width, height, data: out };
    },
  }),
});

/** Kinds the model backend can run. Anything else is unsupported. */
export const MODEL_KINDS = Object.freeze(Object.keys(MODEL_OPS));

/**
 * Apply a model operation to a frame, returning a new frame.
 * Throws on an unknown kind — callers check `MODEL_KINDS` first.
 */
export function applyModelOp(kind, frame) {
  const op = MODEL_OPS[kind];
  if (!op) throw new Error(`unsupported model op: ${kind}`);
  return op.apply(frame);
}
