/** Retained evidence is useful, but a pending, failed or disconnected check is never current. */
export function PreviousCheck({ failed, empty }: { failed: boolean; empty: boolean }) {
  return (
    <>
      <p role="status">
        {failed
          ? 'Could not check with the current service.'
          : 'Awaiting a check with the current service.'}{' '}
        Showing {empty ? 'the previous check' : 'previous findings'}.
      </p>
      {empty && <p>Previous check found no warnings.</p>}
    </>
  );
}
