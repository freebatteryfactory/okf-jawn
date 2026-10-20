/**
 * Where something the reader shows lies in the original, each place with how it was found (SPEC
 * section 5: "The interface never presents an inferred location with the certainty of a direct
 * one"). Every location is worded by `describeLocation`: a direct one as recorded from the
 * document itself, an inferred one as "Inferred location, not certain", found by matching text or
 * layout, and an unresolved one with why it is unknown. Nothing is shown for an empty list: the
 * citation then names the selection alone.
 */
import type { SourceLocation } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { describeLocation } from '../documents/describe';

export function Locations({
  locations,
  label,
}: {
  locations: readonly SourceLocation[];
  /** What the list is, for assistive technology: "Where this reading lies in the original". */
  label: string;
}) {
  if (locations.length === 0) return null;
  return (
    <ul className="locations" aria-label={label}>
      {keyed(locations, (location) => JSON.stringify(location)).map(({ key, value: location }) => {
        const { label: provenance, text } = describeLocation(location);
        return (
          <li key={key} className={`location location-${location.provenance}`}>
            <span className="location-label">{provenance}:</span> {text}
          </li>
        );
      })}
    </ul>
  );
}

/** The page a location names, when it names one; an inferred page is still only inferred. */
export function pageOf(location: SourceLocation): number | null {
  if (location.provenance === 'unresolved') return null;
  const { locator } = location;
  switch (locator.kind) {
    case 'page':
      return locator.page_no;
    case 'region':
      return locator.region.page_no;
    case 'cells':
      return null;
  }
}
