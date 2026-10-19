/**
 * The Explorer's `ViewData`: a View's two reads as the generated client's `read_item` (a cited
 * source) and `get_object` (one block of a retained dataset), through the application's configured
 * client unless another is given.
 *
 * A provider is transport only (`views/data.ts`): each request is handed on exactly as the views
 * components built it, and each answer handed back as the client read it; the views side reads it
 * with the generated schema, checks the revision and the bytes, and never trusts this side for
 * either. A refusal, or a request that never reached the service, rejects with an `Error` whose
 * message is the failure in plain words (`errors/failure.ts`) followed by the service's own, since
 * the views side shows that message where the source or the dataset would have been.
 */
import type { Client } from '../../api/generated/client';
import { getObject, readItem } from '../../api/generated/sdk.gen';
import { classifyFailure, messageFor } from '../errors/failure';
import type { ViewData } from '../views/data';

/** What the generated client answers: its data, or the error the service sent. */
interface Answered<T> {
  data?: T | undefined;
  error?: unknown;
}

/** A failure in plain words, then the service's own message when it gave one. */
export function failureWords(thrown: unknown): string {
  const failure = classifyFailure(thrown);
  const { title } = messageFor(failure);
  const said = failure.kind === 'api' ? failure.error.message.trim() : '';
  return said.length > 0 ? `${title} The service said: “${said}”` : title;
}

/** The answer's data, or its refusal as an `Error` in words. */
async function answer<T>(asked: () => Promise<Answered<T>>): Promise<T> {
  let answered: Answered<T>;
  try {
    answered = await asked();
  } catch (cause) {
    throw new Error(failureWords(cause));
  }
  if (answered.error !== undefined || answered.data === undefined)
    throw new Error(failureWords(answered.error));
  return answered.data;
}

/** A View's reads over the generated client. */
export function explorerViewData(client?: Client): ViewData {
  const using = client === undefined ? {} : { client };
  return {
    readItem: (request) => answer(() => readItem({ ...using, body: request })),
    getObject: (request) => answer(() => getObject({ ...using, body: request })),
  };
}

/**
 * The reads over the application's configured client, as one value: the views components read a
 * View again whenever their provider changes, so every Explorer page gives them this one.
 */
export const workspaceViewData: ViewData = explorerViewData();
