/** Original, inert rule evidence with the same pinned citations and location words as reads. */
import { Link } from '@tanstack/react-router';
import type { InstructionEvidence } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { describeLocation, describeSelection, short } from '../documents/describe';

export function InstructionFindings({ evidence }: { evidence: InstructionEvidence }) {
  return (
    <div className="instruction-findings">
      <p>
        {evidence.language === 'en'
          ? 'English-only rules: instructions in other languages can be missed.'
          : `Rules report language "${evidence.language}"; coverage in other languages is not established.`}{' '}
        Findings are advisory; false positives are possible.
      </p>
      <ol aria-label="Instruction-like findings">
        {keyed(evidence.findings, (finding) => JSON.stringify(finding)).map(
          ({ key, value: finding }) => {
            const source = finding.source;
            const locations = source.locations ?? [];
            return (
              <li key={key}>
                <p>
                  Rule <code>{finding.rule_id}</code>, rule set{' '}
                  <code>{finding.rule_set_version}</code>.
                </p>
                <blockquote style={{ whiteSpace: 'pre-wrap' }}>{finding.passage}</blockquote>
                <p>
                  <Link
                    to="/w/$workspaceId/i/$itemId"
                    params={{ workspaceId: source.workspace_id, itemId: source.item_id }}
                    search={{
                      rev: source.revision,
                      ...(source.selection.kind === 'all' ? {} : { sel: source.selection }),
                    }}
                  >
                    Open cited passage
                  </Link>{' '}
                  in <code>{source.path}</code>, {describeSelection(source.selection)}, at revision{' '}
                  <code title={source.revision}>{short(source.revision)}</code>.
                </p>
                {source.digest && (
                  <p>
                    Conversion record <code title={source.digest}>{short(source.digest)}</code>.
                  </p>
                )}
                {locations.length === 0 ? (
                  <p>No location in the original is recorded.</p>
                ) : (
                  <ul aria-label="Where the passage is in the original">
                    {keyed(locations, (location) => JSON.stringify(location)).map(
                      ({ key: locationKey, value: location }) => {
                        const described = describeLocation(location);
                        return (
                          <li key={locationKey}>
                            <span>{described.label}:</span> {described.text}
                          </li>
                        );
                      },
                    )}
                  </ul>
                )}
              </li>
            );
          },
        )}
      </ol>
      {evidence.omitted_findings > 0 && (
        <p>
          {evidence.omitted_findings === 1
            ? '1 additional finding was omitted.'
            : `${evidence.omitted_findings} additional findings were omitted.`}
        </p>
      )}
    </div>
  );
}
