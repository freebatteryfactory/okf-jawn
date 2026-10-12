/**
 * Show a wire instant (UTC, RFC 3339) in the reader's own locale and time zone.
 *
 * The wire value stays the machine-readable `dateTime`; only the visible text is converted. A
 * value is converted only when it is exactly the contract's `Timestamp` (the generated
 * `zTimestamp`): anything else, such as a date without a time or a day/month spelling, is shown
 * exactly as received and flagged, never guessed into another day or time.
 */
import { zTimestamp } from '../api/generated/zod.gen';

const INSTANT_FORMAT: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' };

/**
 * Format an RFC 3339 instant for display. `locale` and `timeZone` default to the reader's own;
 * they are parameters only so the conversion itself can be checked.
 */
export function formatInstant(value: string, locale?: string, timeZone?: string): string | null {
  if (!zTimestamp.safeParse(value).success) return null;
  const millis = Date.parse(value);
  if (Number.isNaN(millis)) return null;
  const options: Intl.DateTimeFormatOptions =
    timeZone === undefined ? INSTANT_FORMAT : { ...INSTANT_FORMAT, timeZone };
  return new Intl.DateTimeFormat(locale, options).format(new Date(millis));
}

export interface InstantProps {
  /** The instant exactly as the server sent it. */
  value: string;
}

export function Instant({ value }: InstantProps) {
  const shown = formatInstant(value);
  if (shown === null) {
    return (
      <time dateTime={value}>
        {value} <span>(not a recognised time; shown as received)</span>
      </time>
    );
  }
  return (
    <time dateTime={value} title={value}>
      {shown}
    </time>
  );
}
