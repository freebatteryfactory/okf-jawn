/** The page policy forbids eval; validation must work without even probing code generation. */
import { expect, it, vi } from 'vitest';

it('validates and refuses data without attempting dynamic code generation', async () => {
  vi.resetModules();
  const { z } = await import('zod');
  const previous = { ...z.config() };
  const codeGeneration = vi.spyOn(globalThis, 'Function').mockImplementation(() => {
    throw new EvalError('The page policy refuses dynamic code generation');
  });
  try {
    await import('../../src/app/schema-runtime');
    const schema = z.object({ revision: z.string().min(1) });
    expect(schema.safeParse({ revision: 'revision-a' }).success).toBe(true);
    expect(schema.safeParse({ revision: '' }).success).toBe(false);
    expect(codeGeneration).not.toHaveBeenCalled();
  } finally {
    codeGeneration.mockRestore();
    z.config({ ...previous, jitless: previous.jitless ?? false });
  }
});
