/**
 * The two OCR fixtures: pages whose text exists only as pixels, drawn by this module.
 *
 * Why it exists: scanned_image_only.pdf and sample_image.png show grey bars and no glyphs
 * (see their generator notes in tests/fixtures/documents/SOURCES.json), so they cannot show
 * whether OCR recovers text. These fixtures can: the words in OCR_FIXTURES are the only
 * source of both the pixels and the expectation, and tests/foundation/harness.test.mjs
 * decodes the committed files and compares them with renderLines() pixel for pixel.
 *
 * The letters are a monoline stroke font authored here (no font file, no licence to carry):
 * each glyph is a list of polylines on a 6-unit cap height, drawn as round-capped strokes
 * with a one-pixel anti-aliased edge. Only the glyphs the fixtures use are defined.
 */

import { deflateSync, inflateSync } from 'node:zlib';

/** Polylines per glyph; x grows right, y grows up, cap height 6, baseline 0. */
const GLYPHS = {
  A: { width: 4, strokes: [[[0, 0], [2, 6], [4, 0]], [[0.8, 2.2], [3.2, 2.2]]] },
  E: { width: 4, strokes: [[[4, 0], [0, 0], [0, 6], [4, 6]], [[0, 3], [3, 3]]] },
  K: { width: 4, strokes: [[[0, 0], [0, 6]], [[4, 6], [0, 2.6]], [[1.3, 3.7], [4, 0]]] },
  L: { width: 4, strokes: [[[0, 6], [0, 0], [4, 0]]] },
  M: { width: 4.4, strokes: [[[0, 0], [0, 6], [2.2, 2.4], [4.4, 6], [4.4, 0]]] },
  N: { width: 4, strokes: [[[0, 0], [0, 6], [4, 0], [4, 6]]] },
  P: { width: 4, strokes: [[[0, 0], [0, 6], [2.8, 6], [3.7, 5.5], [4, 4.55], [3.7, 3.6], [2.8, 3.1], [0, 3.1]]] },
  R: {
    width: 4,
    strokes: [
      [[0, 0], [0, 6], [2.8, 6], [3.7, 5.5], [4, 4.55], [3.7, 3.6], [2.8, 3.1], [0, 3.1]],
      [[2.2, 3.1], [4, 0]],
    ],
  },
  T: { width: 4, strokes: [[[0, 6], [4, 6]], [[2, 6], [2, 0]]] },
  W: { width: 5, strokes: [[[0, 6], [1.2, 0], [2.5, 4.5], [3.8, 0], [5, 6]]] },
  X: { width: 4, strokes: [[[0, 0], [4, 6]], [[0, 6], [4, 0]]] },
  Y: { width: 4, strokes: [[[0, 6], [2, 3], [4, 6]], [[2, 3], [2, 0]]] },
  4: { width: 4, strokes: [[[3, 0], [3, 6], [0, 2], [4, 2]]] },
  7: { width: 4, strokes: [[[0, 6], [4, 6], [1.5, 0]]] },
};

const CAP_HEIGHT = 6;
const LETTER_GAP = 1.6;
const SPACE = 3;
/** Half the stroke width, in glyph units. */
const STROKE_RADIUS = 0.42;

/** What each OCR fixture shows. `unit` is pixels per glyph unit; `lines` hold text and the cap-top position. */
export const OCR_FIXTURES = {
  'scanned_text.pdf': {
    kind: 'pdf',
    width: 612,
    height: 792,
    unit: 5,
    lines: [
      { text: 'WATER METER', left: 72, top: 96 },
      { text: 'TAX YEAR 47', left: 72, top: 168 },
    ],
  },
  'text_image.png': {
    kind: 'png',
    width: 480,
    height: 160,
    unit: 6,
    lines: [
      { text: 'PLANT 74', left: 40, top: 28 },
      { text: 'KEPT LATE', left: 40, top: 96 },
    ],
  },
};

/** The words a fixture shows, in reading order. */
export function fixtureWords(spec) {
  return spec.lines.flatMap((line) => line.text.split(/\s+/).filter(Boolean));
}

function distanceToSegment(px, py, [ax, ay], [bx, by]) {
  const dx = bx - ax;
  const dy = by - ay;
  const length2 = dx * dx + dy * dy;
  const t = length2 === 0 ? 0 : Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / length2));
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

/** 8-bit grey raster (0 black, 255 white), row-major, of `spec`. Deterministic: IEEE arithmetic only. */
export function renderLines(spec) {
  const { width, height, unit } = spec;
  const gray = new Uint8Array(width * height).fill(255);
  const radius = STROKE_RADIUS * unit;
  for (const line of spec.lines) {
    let cursor = line.left;
    for (const char of line.text) {
      if (char === ' ') {
        cursor += SPACE * unit;
        continue;
      }
      const glyph = GLYPHS[char];
      if (!glyph) throw new Error(`ocr-fixture: no glyph for ${JSON.stringify(char)}`);
      // Glyph units to pixels: y is flipped so the cap top sits at line.top.
      const segments = glyph.strokes.flatMap((stroke) =>
        stroke.slice(1).map((point, index) => [
          [cursor + stroke[index][0] * unit, line.top + (CAP_HEIGHT - stroke[index][1]) * unit],
          [cursor + point[0] * unit, line.top + (CAP_HEIGHT - point[1]) * unit],
        ]),
      );
      const x0 = Math.max(0, Math.floor(cursor - radius - 1));
      const x1 = Math.min(width - 1, Math.ceil(cursor + glyph.width * unit + radius + 1));
      const y0 = Math.max(0, Math.floor(line.top - radius - 1));
      const y1 = Math.min(height - 1, Math.ceil(line.top + CAP_HEIGHT * unit + radius + 1));
      for (let y = y0; y <= y1; y += 1) {
        for (let x = x0; x <= x1; x += 1) {
          let nearest = Infinity;
          for (const [a, b] of segments) nearest = Math.min(nearest, distanceToSegment(x + 0.5, y + 0.5, a, b));
          const coverage = Math.max(0, Math.min(1, radius - nearest + 0.5));
          const value = Math.round(255 * (1 - coverage));
          const at = y * width + x;
          if (value < gray[at]) gray[at] = value;
        }
      }
      cursor += (glyph.width + LETTER_GAP) * unit;
    }
  }
  return gray;
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes) {
  let c = 0xffffffff;
  for (const byte of bytes) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(tag, data) {
  const body = Buffer.concat([Buffer.from(tag, 'latin1'), data]);
  const head = Buffer.alloc(4);
  head.writeUInt32BE(data.length);
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(body));
  return Buffer.concat([head, body, tail]);
}

/** An 8-bit greyscale PNG (colour type 0, filter 0 on every row). */
export function encodePng(spec, gray) {
  const { width, height } = spec;
  const rows = Buffer.alloc((width + 1) * height);
  for (let y = 0; y < height; y += 1) {
    rows.set(gray.subarray(y * width, (y + 1) * width), y * (width + 1) + 1);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 0, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', header),
    pngChunk('IDAT', deflateSync(rows, { level: 9 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

/** A one-page PDF-1.4 whose only content is the raster as a Flate-compressed DeviceGray image; no text operators. */
export function encodePdf(spec, gray) {
  const { width, height } = spec;
  const image = deflateSync(Buffer.from(gray), { level: 9 });
  const content = `q ${width} 0 0 ${height} 0 0 cm /Im0 Do Q`;
  const objects = [
    Buffer.from('<< /Type /Catalog /Pages 2 0 R >>'),
    Buffer.from('<< /Type /Pages /Kids [3 0 R] /Count 1 >>'),
    Buffer.from(
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${width} ${height}] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>`,
    ),
    Buffer.from(`<< /Length ${content.length} >>\nstream\n${content}\nendstream`),
    Buffer.concat([
      Buffer.from(
        `<< /Type /XObject /Subtype /Image /Width ${width} /Height ${height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length ${image.length} >>\nstream\n`,
      ),
      image,
      Buffer.from('\nendstream'),
    ]),
  ];
  const parts = [Buffer.from('%PDF-1.4\n%\xe2\xe3\xcf\xd3\n', 'latin1')];
  let length = parts[0].length;
  const offsets = [];
  objects.forEach((body, index) => {
    offsets.push(length);
    const object = Buffer.concat([Buffer.from(`${index + 1} 0 obj\n`), body, Buffer.from('\nendobj\n')]);
    parts.push(object);
    length += object.length;
  });
  const xref = [`xref\n0 ${objects.length + 1}\n`, '0000000000 65535 f \n'];
  for (const offset of offsets) xref.push(`${String(offset).padStart(10, '0')} 00000 n \n`);
  xref.push(`trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${length}\n%%EOF\n`);
  parts.push(Buffer.from(xref.join('')));
  return Buffer.concat(parts);
}

/** The bytes of one fixture. */
export function encodeFixture(spec) {
  const gray = renderLines(spec);
  return spec.kind === 'png' ? encodePng(spec, gray) : encodePdf(spec, gray);
}

/** The grey raster a committed fixture file holds; throws when the file is not one this module could have written. */
export function decodeFixture(spec, bytes) {
  const { width, height } = spec;
  if (spec.kind === 'png') {
    const idat = bytes.indexOf('IDAT', 8, 'latin1');
    if (idat < 4) throw new Error('ocr-fixture: PNG has no IDAT chunk');
    const length = bytes.readUInt32BE(idat - 4);
    const rows = inflateSync(bytes.subarray(idat + 4, idat + 4 + length));
    if (rows.length !== (width + 1) * height) throw new Error('ocr-fixture: PNG raster has another size');
    const gray = new Uint8Array(width * height);
    for (let y = 0; y < height; y += 1) {
      if (rows[y * (width + 1)] !== 0) throw new Error('ocr-fixture: PNG row uses a filter this module never writes');
      gray.set(rows.subarray(y * (width + 1) + 1, (y + 1) * (width + 1)), y * width);
    }
    return gray;
  }
  const marker = Buffer.from('/Filter /FlateDecode /Length ', 'latin1');
  const at = bytes.indexOf(marker);
  if (at < 0) throw new Error('ocr-fixture: PDF has no Flate image stream');
  const length = Number(/^\d+/.exec(bytes.subarray(at + marker.length, at + marker.length + 12).toString('latin1'))?.[0]);
  const start = bytes.indexOf('stream\n', at, 'latin1') + 'stream\n'.length;
  const gray = inflateSync(bytes.subarray(start, start + length));
  if (gray.length !== width * height) throw new Error('ocr-fixture: PDF image raster has another size');
  return new Uint8Array(gray);
}
