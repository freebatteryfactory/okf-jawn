/**
 * The page's level-one heading, where keyboard focus goes when nothing nearer can take it, is the
 * application's own: never the `#` heading of a document the reader shows.
 */

import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { pageHeading } from '../../src/lib/focus';

const documentTitle = (
  <article>
    <h1>A document’s own title</h1>
  </article>
);
const workspaceName = <h1 tabIndex={-1}>The workspace</h1>;

describe('the page’s level-one heading', () => {
  it('is the application’s, whether a document’s own level-one heading comes after it or before it', () => {
    const view = render(
      <>
        {workspaceName}
        {documentTitle}
      </>,
    );
    expect(pageHeading()?.textContent).toBe('The workspace');
    view.unmount();
    render(
      <>
        {documentTitle}
        {workspaceName}
      </>,
    );
    expect(pageHeading()?.textContent).toBe('The workspace');
  });

  it('is not a document’s, even where the page has no other', () => {
    render(documentTitle);
    expect(pageHeading()).toBeNull();
  });
});
