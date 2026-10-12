/**
 * Where keyboard focus goes when nothing nearer can take it: the page's level-one heading.
 *
 * Each page of the application has one that takes focus from script (`tabindex="-1"`): the
 * workspace list's, the Explorer's (the workspace's name) and the one of the page shown before the
 * Explorer. The reader renders a document's own `#` heading as a level-one heading too; that one
 * is the document's, carries no tabindex, and is not the page's.
 */

/** The page's level-one heading, when the page has one that takes focus from script. */
export function pageHeading(): HTMLElement | null {
  return document.querySelector<HTMLElement>('h1[tabindex="-1"]');
}
