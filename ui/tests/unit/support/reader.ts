/**
 * The reader, the viewers and corrections (UI M4a): the operations they ask, added to the mocked
 * client's contract so every request and answer of a test is held to the generated schemas. The
 * byte read (`download_object`) and the sandbox (`serve_sandbox_representation`) are transports,
 * not operations: a test answers their bytes with its own fetch, as `byte-read.test.ts` does.
 */
import {
  zCorrectDigestRequest,
  zCreateSandboxCapabilityRequest,
  zJob,
  zMutationResult,
  zRedigestRequest,
  zSandboxCapability,
} from '../../../src/api/generated/zod.gen';
import { CONTRACT } from './api';

export const CORRECT_DIGEST = '/api/imports/correct-digest';
export const REDIGEST_ITEM = '/api/imports/redigest-item';
export const CREATE_SANDBOX_CAPABILITY = '/api/reads/create-sandbox-capability';

Object.assign(CONTRACT, {
  [CORRECT_DIGEST]: { request: zCorrectDigestRequest, response: zMutationResult },
  [REDIGEST_ITEM]: { request: zRedigestRequest, response: zJob },
  [CREATE_SANDBOX_CAPABILITY]: {
    request: zCreateSandboxCapabilityRequest,
    response: zSandboxCapability,
  },
});
