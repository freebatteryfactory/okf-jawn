/**
 * An item's properties, read-only, exactly as stored: unknown extension values are shown, not
 * dropped, and no value is reinterpreted (a document date stays the text the file holds).
 */
const APP_HEADER_KEY = 'okf_jawn';

export interface PropertiesProps {
  properties: Record<string, unknown>;
}

function Value({ value }: { value: unknown }) {
  if (value === null) return <span className="empty-value">(empty)</span>;
  if (typeof value === 'string') return <span>{value}</span>;
  if (typeof value === 'number' || typeof value === 'boolean') return <span>{String(value)}</span>;
  return <pre>{JSON.stringify(value, null, 2)}</pre>;
}

export function Properties({ properties }: PropertiesProps) {
  const entries = Object.entries(properties);
  return (
    <details className="properties" open>
      <summary>Properties ({entries.length})</summary>
      {entries.length === 0 ? (
        <p>This item has no properties.</p>
      ) : (
        <dl>
          {entries.map(([key, value]) => (
            <div key={key} className="property">
              <dt>
                {key}
                {key === APP_HEADER_KEY ? ' (kept by okf-jawn; not editable)' : ''}
              </dt>
              <dd>
                <Value value={value} />
              </dd>
            </div>
          ))}
        </dl>
      )}
    </details>
  );
}
