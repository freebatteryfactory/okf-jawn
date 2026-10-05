/** The generated MCP App resource declaration is the only source of the bundle manifest. */
import { z } from 'zod';

const domains = z.array(z.string());
const resourceSchema = z.strictObject({
  uri: z.string().min(1),
  name: z.string().regex(/^[a-z][a-z0-9-]*$/),
  mimeType: z.string().min(1),
  _meta: z.strictObject({
    ui: z.strictObject({
      csp: z.strictObject({ connectDomains: domains, resourceDomains: domains }),
    }),
  }),
});
const declarationSchema = z.strictObject({ resources: z.tuple([resourceSchema]) });

/** One declared App resource, in the shape `rmcp::model::Resource` deserializes. */
export type AppResource = z.infer<typeof resourceSchema>;

/**
 * Return the single declared resource. Throws when the declaration is malformed, when its uri is
 * not the file this build writes, or when its mimeType is not the installed SDK's.
 */
export function parseAppResource(
  declaration: unknown,
  source: string,
  sdkMimeType: string,
): AppResource {
  const parsed = declarationSchema.safeParse(declaration);
  if (!parsed.success) {
    throw new Error(`${source} is not an MCP App declaration: ${parsed.error.message}`);
  }
  const [resource] = parsed.data.resources;
  const builtUri = `ui://okf-jawn/${resource.name}.html`;
  if (resource.uri !== builtUri) {
    throw new Error(`${source} uri ${resource.uri} !== built resource ${builtUri}`);
  }
  if (resource.mimeType !== sdkMimeType) {
    throw new Error(`${source} mimeType ${resource.mimeType} !== SDK ${sdkMimeType}`);
  }
  return resource;
}

/** The manifest entry for the built file: the declaration plus the bytes' length and digest. */
export function manifestEntry(resource: AppResource, byteLength: number, sha256: string) {
  return { ...resource, byteLength, sha256 };
}
