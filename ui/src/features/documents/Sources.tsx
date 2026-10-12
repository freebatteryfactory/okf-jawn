/**
 * Present an item's citations and source appearance exactly as the sources operation returned
 * them: each citation with its selection, digest and located places in the original, each place
 * with how it was obtained, and every declared `sources` entry with what it resolved to.
 *
 * Shared with the MCP App, so it takes only the operation's result and needs no router or cache.
 */
import { useId } from 'react';
import type { z } from 'zod';
import type { zGetSourcesResponse, zSourceLocation } from '../../api/generated/zod.gen';
import { Instant } from '../../lib/instant';
import { keyed } from '../../lib/keys';
import {
  describeLocation,
  describeOutcome,
  describeSelection,
  describeWarning,
  short,
  TEXT_ORIGIN,
  UNCITED,
} from './describe';

export interface SourcesProps {
  result: z.infer<typeof zGetSourcesResponse>;
}

export function Sources({ result }: SourcesProps) {
  const declared = result.declared ?? [];
  const heading = useId();
  return (
    <section aria-labelledby={heading} className="sources">
      <h2 id={heading}>Sources</h2>
      <p>
        At revision <code title={result.revision}>{short(result.revision)}</code>
      </p>
      <h3>Citations</h3>
      {result.sources.length === 0 ? (
        <p>This item cites no sources at this revision.</p>
      ) : (
        <ol className="citations">
          {keyed(result.sources, (source) => JSON.stringify(source)).map(
            ({ key, value: source }) => (
              <li key={key}>
                <p>
                  <strong>{source.path}</strong> at{' '}
                  <code title={source.revision}>{short(source.revision)}</code>
                </p>
                <p>Cites {describeSelection(source.selection)}.</p>
                {source.digest ? (
                  <p>
                    Conversion record <code title={source.digest}>{short(source.digest)}</code>
                  </p>
                ) : null}
                <Locations locations={source.locations ?? []} />
              </li>
            ),
          )}
        </ol>
      )}
      {declared.length > 0 && (
        <>
          <h3>Declared in this file</h3>
          <ol className="declared">
            {keyed(declared, (entry) => JSON.stringify(entry)).map(({ key, value: entry }) => (
              <li key={key}>
                <code>{JSON.stringify(entry.entry)}</code>
                <p>
                  {entry.outcome.kind === 'cited'
                    ? `Cited as citation ${entry.outcome.index + 1}.`
                    : `Not a citation: ${UNCITED[entry.outcome.reason]}.`}
                </p>
              </li>
            ))}
          </ol>
        </>
      )}
      {result.appearance && <Appearance appearance={result.appearance} />}
    </section>
  );
}

function Locations({ locations }: { locations: readonly z.infer<typeof zSourceLocation>[] }) {
  if (locations.length === 0) return <p>No location in the original is recorded.</p>;
  return (
    <ul className="locations" aria-label="Where this is in the original">
      {keyed(locations, (location) => JSON.stringify(location)).map(({ key, value: location }) => {
        const { label, text } = describeLocation(location);
        return (
          <li key={key} className={`location location-${location.provenance}`}>
            <span className="location-label">{label}:</span> {text}
          </li>
        );
      })}
    </ul>
  );
}

type Appearance = NonNullable<SourcesProps['result']['appearance']>;

function Appearance({ appearance }: { appearance: Appearance }) {
  const { extraction } = appearance;
  return (
    <>
      <h3>Original file</h3>
      <dl className="appearance">
        <dt>Observed as</dt>
        <dd>
          <ul>
            {keyed(appearance.names, (name) => JSON.stringify(name)).map(({ key, value: name }) => (
              <li key={key}>
                {name.folder ? `${name.folder}/` : ''}
                {name.filename}, <Instant value={name.observed_at} />, supplied by{' '}
                {name.supplied_by}
              </li>
            ))}
          </ul>
        </dd>
        <dt>Type</dt>
        <dd>{appearance.media_type}</dd>
        <dt>Size</dt>
        <dd>
          {/^\d+$/.test(appearance.size)
            ? BigInt(appearance.size).toLocaleString()
            : appearance.size}{' '}
          bytes
        </dd>
        <dt>Original bytes</dt>
        <dd>
          <code title={appearance.object}>{short(appearance.object)}</code>
        </dd>
        <dt>Conversion</dt>
        <dd>
          <p>{describeOutcome(extraction.outcome)}</p>
          <p>{TEXT_ORIGIN[extraction.text_origin]}</p>
          {extraction.corrected && <p>A person’s correction is shown over the converted text.</p>}
          {extraction.converter ? (
            <p>
              Converter: {extraction.converter.name} {extraction.converter.version}
            </p>
          ) : null}
          {extraction.warnings.length > 0 && (
            <ul>
              {keyed(extraction.warnings, (warning) => JSON.stringify(warning)).map(
                ({ key, value: warning }) => (
                  <li key={key}>{describeWarning(warning)}</li>
                ),
              )}
            </ul>
          )}
        </dd>
      </dl>
    </>
  );
}
