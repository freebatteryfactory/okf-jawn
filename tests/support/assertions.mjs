/** Assertions describe required meaning without importing the implementation under test. */
import assert from 'node:assert/strict';
export function reviewCoversRevision(review, expectedRevision) {
  assert.equal(review.source.revision,expectedRevision,'review must name the revision actually reviewed');
  assert.ok(['current','changed','imported','unreviewed'].includes(review.coverage),'known coverage');
}
export function readResolvedRevision(result, requestedRevision) {
  assert.equal(result.source.revision,requestedRevision,'read must not silently advance a pinned revision');
  assert.equal(typeof result.markdown,'string');
  assert.equal(typeof result.truncated,'boolean');
  if(result.truncated)assert.ok(result.next_cursor,'partial result needs continuation');
}
