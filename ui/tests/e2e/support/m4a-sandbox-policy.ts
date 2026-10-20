/** A5 console attribution: keep sandbox refusals as evidence without exempting page violations. */
export interface PolicyConsoleLine {
  text: string;
  source: string;
}

interface Attribution {
  sandboxOrigin: string;
  pixelUrl: string;
  /** True only after this screen verified the hostile file loaded in its capability frame. */
  hostileFrameShown: boolean;
  parentViolations: ReadonlyArray<{ directive: string; blocked: string }>;
}

export function partitionPolicyConsole(lines: readonly PolicyConsoleLine[], context: Attribution) {
  const application: PolicyConsoleLine[] = [];
  const sandbox: Array<PolicyConsoleLine & { attribution: string }> = [];
  const pixelOrigin = new URL(context.pixelUrl).origin;
  const parentPixelRefused = context.parentViolations.some(
    (violation) =>
      violation.directive.startsWith('img-src') &&
      (violation.blocked === context.pixelUrl || violation.blocked === pixelOrigin),
  );
  const expectedPixelLine = `Refused to load ${context.pixelUrl} because it does not appear in the img-src directive of the Content Security Policy.`;
  for (const line of lines) {
    if (line.source.startsWith(`${context.sandboxOrigin}/`)) {
      sandbox.push({ ...line, attribution: 'console source is the sandbox origin' });
    } else if (
      context.hostileFrameShown &&
      line.source === '' &&
      line.text === expectedPixelLine &&
      !parentPixelRefused
    ) {
      sandbox.push({
        ...line,
        attribution:
          'exact WebKit hostile-file pixel refusal; capability frame loaded; no matching parent violation',
      });
    } else {
      application.push(line);
    }
  }
  return { application, sandbox };
}
