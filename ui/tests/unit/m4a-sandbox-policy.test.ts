/** A5's expected sandbox denial never hides the same target denied in the parent page. */
import { describe, expect, it } from 'vitest';
import { partitionPolicyConsole } from '../e2e/support/m4a-sandbox-policy';

const pixelUrl = 'http://elsewhere.okf.test/pixel.png';
const line = {
  text: 'Refused to load http://elsewhere.okf.test/pixel.png because it does not appear in the img-src directive of the Content Security Policy.',
  source: '',
};
const context = {
  sandboxOrigin: 'http://sandbox.okf.test:7712',
  pixelUrl,
  hostileFrameShown: true,
  parentViolations: [],
};

describe('A5 sandbox console attribution', () => {
  it('records the exact unlocated WebKit fixture refusal as sandbox evidence', () => {
    const found = partitionPolicyConsole([line], context);
    expect(found.application).toEqual([]);
    expect(found.sandbox).toEqual([
      {
        ...line,
        attribution:
          'exact WebKit hostile-file pixel refusal; capability frame loaded; no matching parent violation',
      },
    ]);
  });

  it.each([pixelUrl, 'http://elsewhere.okf.test'])(
    'keeps a matching parent image violation (%s) in the application inventory',
    (blocked) => {
      const found = partitionPolicyConsole([line], {
        ...context,
        parentViolations: [{ directive: 'img-src', blocked }],
      });
      expect(found.application).toEqual([line]);
      expect(found.sandbox).toEqual([]);
    },
  );

  it('requires the verified hostile frame and preserves every other unlocated refusal', () => {
    expect(
      partitionPolicyConsole([line], { ...context, hostileFrameShown: false }).application,
    ).toEqual([line]);
    const other = { ...line, text: line.text.replace('/pixel.png', '/another.png') };
    expect(partitionPolicyConsole([other], context).application).toEqual([other]);
    const script = {
      source: '',
      text: 'Refused to execute inline script because of Content Security Policy.',
    };
    expect(partitionPolicyConsole([script], context).application).toEqual([script]);
  });
});
