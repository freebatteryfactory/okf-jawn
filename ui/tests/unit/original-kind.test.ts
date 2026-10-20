/**
 * Which viewer an item's original goes to (ruling A2, A3), by the media type the service sniffed
 * (`SourceAppearance.media_type`): PDF.js for a PDF, the image viewer for an image, the sandbox
 * only for `text/html` exactly (a derived HTML object is never framed here), tables for a
 * spreadsheet, the converted text with the original's access for an Office file, the text viewer
 * for plain text and code, and words with the original's access for anything else.
 */
import { describe, expect, it } from 'vitest';
import { originalKind } from '../../src/features/viewers/original-kind';

describe('the viewer an original goes to', () => {
  it.each([
    ['application/pdf', 'pdf'],
    ['image/png', 'image'],
    ['image/jpeg', 'image'],
    ['image/webp', 'image'],
    ['text/html', 'html'],
    ['text/html; charset=utf-8', 'html'],
    ['TEXT/HTML', 'html'],
    ['text/csv', 'spreadsheet'],
    ['application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', 'spreadsheet'],
    ['application/vnd.ms-excel', 'spreadsheet'],
    ['application/vnd.oasis.opendocument.spreadsheet', 'spreadsheet'],
    ['application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'office'],
    ['application/vnd.openxmlformats-officedocument.presentationml.presentation', 'office'],
    ['application/msword', 'office'],
    ['application/vnd.oasis.opendocument.text', 'office'],
    ['text/plain', 'text'],
    ['text/markdown', 'text'],
    ['application/json', 'text'],
    ['application/zip', 'other'],
    ['application/octet-stream', 'other'],
  ])('%s goes to %s', (mediaType, kind) => {
    expect(originalKind(mediaType)).toBe(kind);
  });

  it('frames nothing in the sandbox but text/html: not XHTML, not SVG', () => {
    expect(originalKind('application/xhtml+xml')).not.toBe('html');
    expect(originalKind('image/svg+xml')).not.toBe('html');
  });
});
