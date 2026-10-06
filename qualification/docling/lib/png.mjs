/**
 * Decode a PNG far enough to say whether it shows anything.
 *
 * A page render that is one flat colour depicts nothing, whatever its size and hash say.
 * `pngInk` inflates the image data, undoes the row filters and counts the pixels that differ
 * from the first pixel. It reads non-interlaced images of 8 or 16 bits per sample in colour
 * types 0, 2, 4 and 6 (grey, RGB, grey + alpha, RGB + alpha): what docling writes for a page
 * (8-bit RGB). Anything else is reported as not decoded with the reason, never as flat or inked.
 *
 * Uses only node:zlib, which the runtime ships; no package is added for this.
 */

import { inflateSync } from 'node:zlib';

const SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
/** Samples per pixel by PNG colour type. */
const SAMPLES = { 0: 1, 2: 3, 4: 2, 6: 4 };

const undecoded = (reason) => ({ decoded: false, reason });

function paeth(left, up, upLeft) {
  const estimate = left + up - upLeft;
  const dLeft = Math.abs(estimate - left);
  const dUp = Math.abs(estimate - up);
  const dUpLeft = Math.abs(estimate - upLeft);
  if (dLeft <= dUp && dLeft <= dUpLeft) return left;
  return dUp <= dUpLeft ? up : upLeft;
}

/** Undo one row's filter in place. `previous` is the row above, already unfiltered, or null. */
function unfilter(type, row, previous, step) {
  for (let i = 0; i < row.length; i += 1) {
    const left = i >= step ? row[i - step] : 0;
    const up = previous ? previous[i] : 0;
    const upLeft = previous && i >= step ? previous[i - step] : 0;
    if (type === 1) row[i] = (row[i] + left) & 0xff;
    else if (type === 2) row[i] = (row[i] + up) & 0xff;
    else if (type === 3) row[i] = (row[i] + ((left + up) >> 1)) & 0xff;
    else if (type === 4) row[i] = (row[i] + paeth(left, up, upLeft)) & 0xff;
  }
}

/**
 * @param {Uint8Array|null|undefined} bytes the file as read
 * @returns {{ decoded: true, width: number, height: number, pixels: number, differing_pixels: number, uniform: boolean, first_pixel: number[] }
 *   | { decoded: false, reason: string }} `first_pixel` holds the raw sample bytes of the top-left pixel
 */
export function pngInk(bytes) {
  if (!bytes || bytes.length < 8 || !SIGNATURE.every((value, index) => bytes[index] === value)) {
    return undecoded('the bytes do not start with the PNG signature');
  }
  const data = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.length);
  let header = null;
  const compressed = [];
  for (let at = 8; at + 12 <= data.length; ) {
    const length = data.readUInt32BE(at);
    const tag = data.toString('latin1', at + 4, at + 8);
    const body = data.subarray(at + 8, at + 8 + length);
    if (body.length !== length) return undecoded(`chunk ${tag} is cut short`);
    if (tag === 'IHDR') header = body;
    else if (tag === 'IDAT') compressed.push(body);
    else if (tag === 'IEND') break;
    at += 12 + length;
  }
  if (!header || header.length !== 13) return undecoded('no IHDR chunk');
  const width = header.readUInt32BE(0);
  const height = header.readUInt32BE(4);
  const [depth, colourType, , , interlace] = header.subarray(8);
  const samples = SAMPLES[colourType];
  if (!samples || (depth !== 8 && depth !== 16) || interlace !== 0) {
    return undecoded(`bit depth ${depth}, colour type ${colourType}, interlace ${interlace} is not a format this reader decodes`);
  }
  if (!(width > 0 && height > 0)) return undecoded('the image has no pixels');
  let raw;
  try {
    raw = inflateSync(Buffer.concat(compressed));
  } catch (error) {
    return undecoded(`image data does not inflate: ${error.message}`);
  }
  const step = samples * (depth / 8);
  const stride = width * step;
  if (raw.length !== (stride + 1) * height) {
    return undecoded(`image data is ${raw.length} bytes; ${width}x${height} at ${step} byte(s) per pixel needs ${(stride + 1) * height}`);
  }
  let previous = null;
  let first = null;
  let differing = 0;
  for (let y = 0; y < height; y += 1) {
    const start = y * (stride + 1);
    const type = raw[start];
    if (type > 4) return undecoded(`row ${y} uses unknown filter ${type}`);
    const row = raw.subarray(start + 1, start + 1 + stride);
    unfilter(type, row, previous, step);
    first ??= Array.from(row.subarray(0, step));
    for (let x = 0; x < stride; x += step) {
      for (let s = 0; s < step; s += 1) {
        if (row[x + s] !== first[s]) {
          differing += 1;
          break;
        }
      }
    }
    previous = row;
  }
  return { decoded: true, width, height, pixels: width * height, differing_pixels: differing, uniform: differing === 0, first_pixel: first };
}
