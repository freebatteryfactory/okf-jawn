/**
 * What a View was drawn from, said under it: its mode, each source with the revision it is shown
 * at and, for a live View, the as-of time (SPEC section 10: "Explicit live mode reports each
 * resolved revision and as-of time"); then every warning of the present or resolve result, in
 * plain words, with the server's message exactly as sent.
 *
 * A warning is read by its code and its `location` only, never by the wording of its message. A
 * location `/bindings/<i>` is a binding of the View's document, named by that binding's name; a
 * warning without one, or with one this View has no binding at, is about the whole View. A code
 * this build does not know is shown with its message, never dropped.
 */
import type { z } from 'zod';
import type { zPresentResponse, zWarning } from '../../api/generated/zod.gen';
import { Instant } from '../../lib/instant';

type PresentResponse = z.infer<typeof zPresentResponse>;
type Warning = z.infer<typeof zWarning>;

/** What each known warning says of the binding (or View) it is about. */
const warningWords: Readonly<Record<string, string>> = {
  dataset_unavailable: 'its data could not be read, so what draws from it is not shown.',
  newer_source:
    'its source has changed since the revision this View cites. The View still shows that revision: newer data is an observation, not proof that the View is wrong.',
  source_removed:
    'its source is no longer in the workspace. The View still shows the revision it cites.',
  live_resolution:
    'its sources were read as the workspace stands now, and a review never covers live data.',
  unknown_unit: 'a unit it declares names no column of its data.',
};

const bindingPointer = /^\/bindings\/(0|[1-9][0-9]*)$/;

/** The binding a warning's location names, if this View has one there. */
export function warnedBinding(
  view: PresentResponse['view'],
  location: Warning['location'],
): string | undefined {
  const index = location ? bindingPointer.exec(location)?.[1] : undefined;
  return index === undefined ? undefined : view.bindings.at(Number(index))?.name;
}

/** Who a warning is about, as its line names it. */
function about(view: PresentResponse['view'], warning: Warning): string {
  const binding = warnedBinding(view, warning.location);
  if (binding !== undefined) return binding;
  return warning.location ? `This View (${warning.location})` : 'This View';
}

/** One warning in plain words: whom it is about, what it means, and what the server said. */
export function warningLine(view: PresentResponse['view'], warning: Warning): string {
  const words = warningWords[warning.code] ?? `the server noted “${warning.code}”.`;
  return `${about(view, warning)}: ${words} The server said: “${warning.message}”`;
}

export function Resolution({ response }: { response: PresentResponse }) {
  const heading = `view-resolution-${response.receipt_id}`;
  return (
    <section aria-labelledby={heading} className="view-resolution">
      <h3 id={heading}>Where this View's data comes from</h3>
      {response.view.mode === 'live' ? (
        <p>
          Live: each source is shown at the revision it resolved to as of{' '}
          <Instant value={response.as_of} />.
        </p>
      ) : (
        <p>Pinned: each source is shown at the revision this View cites, and does not refresh.</p>
      )}
      {response.resolved_bindings.length === 0 ? (
        <p>This View cites no sources.</p>
      ) : (
        <ul aria-label="Sources read">
          {response.resolved_bindings.map((binding) => (
            <li key={binding.name}>
              {binding.name}: {binding.source.path} at revision{' '}
              <code>{binding.source.revision}</code>
            </li>
          ))}
        </ul>
      )}
      {response.warnings.length > 0 && (
        <ul aria-label="Warnings">
          {response.warnings.map((warning, index) => (
            <li key={JSON.stringify([index, warning.code])}>
              {warningLine(response.view, warning)}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
