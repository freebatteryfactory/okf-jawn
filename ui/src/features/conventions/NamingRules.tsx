/**
 * The naming rules of the workspace: a form over them and an advanced YAML view of the same rules,
 * saving them, a preview of what they would rename, and applying a preview the person has seen
 * (SPEC section 6).
 *
 * One value is edited, starting from the saved rules of the version the workspace shows. The form
 * and the YAML view are two views of it, one on screen at a time: a change in the form is written
 * into the YAML, and YAML that can be read becomes what the form shows. The value is kept as it
 * is: a key neither view knows is never dropped, and a form edit leaves every key where the YAML
 * had it (`keepOrder`). Comments are not kept: the service stores the rules as data, so a comment
 * never reaches it, and the YAML view says so before anything is typed. YAML that cannot be read changes nothing: the
 * value stays the last one that could, the text stays as typed (through a change of view too),
 * and its problems are listed with their lines. Nothing is sent while the YAML cannot be read or
 * the value has a problem by the service's schema: the problems are shown at their fields and in
 * a list, and Save and Preview say so instead of asking the service.
 *
 * The YAML is read off the page's thread, a moment after it is typed (`lib/yaml-reading.ts`).
 * While the text is not read yet (it waits for the typing to pause, or is being read), the rules
 * on the page are older than the text, so nothing is sent or applied: a press of Save, Preview,
 * "Apply this preview…" or "Rename" starts the reading at once, does nothing else, and the status
 * line says why. The form is not drawn meanwhile, and leaving asks, as for an edit. A text whose
 * reading takes longer than a second is ended and is then a text that cannot be read: its
 * problem says that it is too complex, and the text stays in the box.
 *
 * A preview writes nothing. It lists each item that would be renamed with its name now and its
 * name after, as a table (read row by row with a screen reader's table keys; the column it is in
 * scrolls from any control), with the service's warnings and the duplicate paths it observed, and
 * the counts the plan gives: the items considered, those that keep their names, and the archived
 * items left out (ruling 10). A preview belongs to the revision and the rules it was made of.
 * When either is no longer what the page shows (the person saved the rules, made another change
 * or refreshed; the rules were edited; the service refused the preview as out of date), it is
 * marked out of date and cannot be applied: the person previews again. Applying asks first, and
 * sends back exactly the plan and the rules of the preview on screen. A renaming that failed is
 * tried again by its notice only while its preview is still the one on screen and can still be
 * applied; otherwise the notice offers no retry and says to preview again (SPEC section 6: "A
 * stale preview must not silently apply to changed source state"). A save that failed is tried
 * again only while the rules on screen are the ones it carried.
 *
 * Saving over rules someone else saved is a choice, never one plain "Save". The edits start
 * from the saved rules of the version shown. When the version shown comes to hold other saved
 * rules while there are edits (a save was refused because the head moved, and the person
 * refreshed), the page shows the rules as they are saved now, read-only, beside the person's own;
 * the save button then reads "Replace the saved rules with mine…", and asks before it sends: the
 * rules are sent whole, and nothing of the others is merged in. The other way out takes the
 * saved rules and drops the edits. Until the rules of the version shown are on the page (they are
 * being read at a new head, or that read failed and shows its retry), nothing is saved at all:
 * the button is unavailable and the line beside it says why, so a save never goes over rules the
 * page has not shown.
 *
 * A save that succeeded leaves nothing unsaved. Once the page shows the rules that save made, the
 * form and the YAML text are what is saved, not edits: leaving asks nothing, and rules someone
 * else saves later are simply followed, with nothing shown beside them and no offer to replace
 * them (the person's older saved rules would go over the newer ones). Text typed after the save
 * was sent, while it was under way too, is not what was saved: it is an edit again, of the rules
 * that save made.
 *
 * Edits that are not saved are not lost silently. While the rules differ from the saved ones, or
 * the YAML view holds text that cannot be read or is not read yet, leaving asks first: "Close the
 * naming rules"; the shell's own ways out (`leaveGuard`): its "Naming rules" button, opening an
 * item, and every navigation that would take the rules off the page, which the shell holds back
 * through the router until the question is answered (Back and Forward, a link to another page, a
 * search result's link); and a reload or a closed browser tab (the browser's own question,
 * `beforeunload`, as for a draft's text). The question opens on "Keep editing", which gives focus
 * back to what asked to leave and tells whoever asked that the person stays.
 *
 * Focus: the panel opens on its heading; a preview puts focus on the preview's heading; the
 * question before renaming opens on the choice that keeps the names, and gives focus back to the
 * button that opened it; after the renaming focus is on the panel's heading. When the question
 * leaves by itself with keyboard focus in it (its preview went out of date: a save pressed just
 * before landed, the service refused the plan), focus goes to the button that opened it, which is
 * then unavailable and says why beside it, else to the panel's heading (`useWatchedPart`). A
 * question that left so is over: it does not open again by itself when the preview is the preview
 * of what the page shows once more. Buttons that can
 * become unavailable with focus use `aria-disabled`.
 */

import { Tabs } from '@base-ui/react/tabs';
import type { RJSFSchema } from '@rjsf/utils';
import { useMutation, useQuery } from '@tanstack/react-query';
import { type RefObject, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { z } from 'zod';
import type { NamingRules, RenamePlan, Revision, Warning } from '../../api/generated/types.gen';
import { takeHeld, useWatchedPart } from '../../lib/focus';
import { keyed } from '../../lib/keys';
import {
  usePausedReading,
  YamlEditor,
  type YamlEditorHandle,
  YamlFindings,
} from '../../lib/YamlEditor';
import { writeYaml, type YamlPlace, type YamlRead } from '../../lib/yaml';
import { readOffThread } from '../../lib/yaml-reading';
import { short } from '../documents/describe';
import { FailureNotice } from '../errors/FailureNotice';
import { useRetry } from '../errors/retry';
import type { HeadChange } from '../explorer/head';
import { ChangeFailure, useChange } from '../tree/ChangeForms';
import { useWorkspaceChanges } from '../tree/context';
import { folderProblem } from '../tree/names';
import { formHolds, RulesForm, rulesProblems } from './RulesForm';
import {
  applyPreview,
  keepOrder,
  type Preview,
  previewRenames,
  problemPlace,
  type Renamed,
  rulesOptions,
  rulesSchemaOptions,
  type StoredRules,
  sameRules,
  sameStored,
  saveRules,
  startingRules,
} from './rules';

/**
 * Leave the naming rules by `proceed`, after a question when edits would be lost. `stay` is
 * called instead when the person chooses to keep editing (or asks to leave another way before
 * answering): whoever asked learns that this leaving is off.
 */
export type LeaveGuard = (proceed: () => void, stay?: () => void) => void;

export interface NamingRulesProps {
  /** Leave the naming rules; the caller puts focus back on what opened them. */
  onClose: () => void;
  /**
   * Set, while the panel is on screen, to its guard: the caller leaves through it, so unsaved
   * edits are asked about first.
   */
  leaveGuard?: RefObject<LeaveGuard | null>;
}

const zSchema = z.record(z.string(), z.unknown());
const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const NOT_READ_YET =
  'The text of the YAML view is still being read. Nothing was sent or changed: press again in a moment.';
const RULES_REFUSED =
  'The service refused it because it does not fit the workspace as it is now. Preview again, then check the rules.';

type UnreadableFile = Extract<StoredRules, { kind: 'unreadable' }>;

const NO_PLACES: Readonly<Record<string, YamlPlace>> = {};

/** `stored` when it is a file that cannot be read as rules. */
function unreadableIn(stored: StoredRules | undefined): UnreadableFile | undefined {
  return stored?.kind === 'unreadable' ? stored : undefined;
}

/**
 * Where the JSON Pointers among the problems of `file` stand in its text. The text is read off
 * the page's thread, as typed YAML is (`readOffThread`): it is untrusted, and can be large. Until
 * that answers, and for a text it could not read, a pointer has no line.
 */
function usePointerPlaces(file: UnreadableFile | undefined): Readonly<Record<string, YamlPlace>> {
  const [found, setFound] = useState<{
    of: UnreadableFile;
    places: Readonly<Record<string, YamlPlace>>;
  } | null>(null);
  useEffect(() => {
    if (file === undefined) return;
    const pointers = file.problems.flatMap((problem) => {
      const place = problemPlace(problem.location);
      return place.kind === 'pointer' ? [place.pointer] : [];
    });
    if (pointers.length === 0) return;
    return readOffThread(
      file.text,
      (read) => setFound({ of: file, places: read.ok ? (read.places ?? NO_PLACES) : NO_PLACES }),
      pointers,
    );
  }, [file]);
  return found !== null && found.of === file ? found.places : NO_PLACES;
}

/**
 * A problem of a stored rules file in words, with the line and column it is at in the file's
 * text when that is known: given as such, or found for its JSON Pointer in `places`.
 */
function placed(
  problem: Warning,
  places: Readonly<Record<string, YamlPlace>>,
): { words: string; place: YamlPlace | null } {
  const where = problemPlace(problem.location);
  if (where.kind === 'line') {
    return {
      words: `Line ${where.line}, column ${where.column}: ${problem.message}`,
      place: { line: where.line, column: where.column },
    };
  }
  if (where.kind === 'pointer') {
    const place = places[where.pointer];
    return place === undefined
      ? { words: `At ${where.pointer}: ${problem.message}`, place: null }
      : {
          words: `Line ${place.line}, column ${place.column} (${where.pointer}): ${problem.message}`,
          place,
        };
  }
  if (where.kind === 'words')
    return { words: `At ${where.words}: ${problem.message}`, place: null };
  return { words: `In the whole file: ${problem.message}`, place: null };
}

/**
 * Why a preview can no longer be applied; null while it is the preview of what the page shows.
 * `refused` is the plan the service refused as out of date, if it refused one.
 */
export function staleReason(
  preview: Preview,
  head: Revision,
  rules: unknown,
  refused: RenamePlan | null,
): string | null {
  if (preview.plan.base_revision !== head) {
    return `The workspace changed since this preview: it now shows revision ${short(head)}, and the preview was made on ${short(preview.plan.base_revision)}.`;
  }
  if (!sameRules(preview.rules, rules)) return 'The rules were changed since this preview.';
  if (preview.plan === refused) {
    return 'The service refused this preview: the workspace changed since it was made.';
  }
  return null;
}

export function NamingRulesPanel({ onClose, leaveGuard }: NamingRulesProps) {
  const { workspaceId, head, canWrite } = useWorkspaceChanges();
  const heading = useId();
  const previewHeading = useId();
  const folderId = useId();
  const folderErrorId = useId();
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const previewRef = useRef<HTMLHeadingElement | null>(null);
  const previewButton = useRef<HTMLButtonElement | null>(null);
  const saveButton = useRef<HTMLButtonElement | null>(null);
  const applyButton = useRef<HTMLButtonElement | null>(null);
  const keepButton = useRef<HTMLButtonElement | null>(null);
  const folderRef = useRef<HTMLInputElement | null>(null);
  const problemsRef = useRef<HTMLDivElement | null>(null);

  const schema = useQuery(rulesSchemaOptions());
  // What the version shown holds as its rules. While they are read again at a new head, and when
  // that read fails, the ones read last stay on screen: a read that fails in the background
  // replaces nothing, and the line above the rules says which version they are of.
  const saved = useQuery(rulesOptions(workspaceId, head));
  const [lastRead, setLastRead] = useState<{ stored: StoredRules; at: Revision } | null>(null);
  if (saved.data !== undefined && (lastRead?.at !== head || lastRead.stored !== saved.data)) {
    setLastRead({ stored: saved.data, at: head });
  }
  const stored = saved.data ?? lastRead?.stored;
  // The saved rules edits start from: with none saved, none at all; a stored file that cannot be
  // read has none (it is repaired as text).
  const savedRules = stored === undefined ? undefined : startingRules(stored);
  // What was saved when the edits started (see `basis` below).
  const [basis, setBasis] = useState<StoredRules | undefined>(undefined);
  // The stored file the edits start from, when it cannot be read as rules: the page is repairing
  // it. Its text is what the YAML view starts from, the form is not offered, and its problems are
  // listed at their lines.
  const fromFile = unreadableIn(basis ?? stored);
  const repairing = fromFile !== undefined;
  const filePlaces = usePointerPlaces(fromFile);
  const schemaFailure = useRetry({
    request: workspaceId,
    failed: schema.isError,
    error: schema.error,
    run: () => schema.refetch(),
    loaded: () => headingRef.current,
  });
  const savedFailure = useRetry({
    request: `${workspaceId} ${head}`,
    failed: saved.isError,
    error: saved.error,
    run: () => saved.refetch(),
    loaded: () => headingRef.current,
  });

  // The rules being edited: undefined while they are the saved ones, untouched.
  const [edited, setEdited] = useState<unknown>(undefined);
  const value = edited === undefined ? savedRules : edited;
  // Which view is on screen, and the YAML as typed: null while it is the value written out.
  const [view, setView] = useState<'form' | 'yaml'>('form');
  const [typed, setTyped] = useState<{ text: string; read: YamlRead } | null>(null);
  const unreadable = typed !== null && !typed.read.ok;
  // The YAML is read a pause after the last change, off the page's thread; `flush` starts reading
  // at once what still waits (at a change of view, and when a press finds text that is not read).
  const reading = usePausedReading((text, read) => {
    // The stored file's own text, typed back as it was, is no edit of it.
    if (fromFile !== undefined && text === fromFile.text) {
      setTyped(null);
      setEdited(undefined);
      return;
    }
    setTyped({ text, read });
    // Only YAML that can be read becomes the rules; otherwise they stay as they were.
    if (read.ok) setEdited(read.value);
  });
  const findingsId = useId();
  const fileNoticeId = useId();
  const shape = zSchema.safeParse(schema.data);
  const form: RJSFSchema | null = shape.success ? shape.data : null;
  const problems = form === null || value === undefined ? [] : rulesProblems(form, value);
  /** `held` as rules that can be sent: without a problem by the service's own schema. */
  const sendableOf = (held: unknown, readable: boolean): NamingRules | null =>
    form !== null && readable && formHolds(held) && rulesProblems(form, held).length === 0
      ? held
      : null;
  const rules = sendableOf(value, !unreadable);
  // Any rules the person's text holds differ from a stored file that cannot be read.
  const changed =
    edited !== undefined &&
    (savedRules === undefined ? stored?.kind === 'unreadable' : !sameRules(edited, savedRules));

  // The saved rules the edits started from. While nothing is edited they follow the rules of the
  // version shown; once there are edits they stay, so other rules saved meanwhile are seen.
  //
  // Text of the person's that is not among the rules yet is an edit too: text that waits to be
  // read or is being read, and text that cannot be read. It was typed over the rules shown then,
  // so they stay what it started from, and when it is read the rules saved meanwhile are seen.
  //
  // The YAML text box takes its text when it mounts. While it holds nothing of the person's it
  // follows the rules shown: when they change (a Refresh loaded other rules) it starts again from
  // them, or an edit made next would be made in the older rules and saved over the newer. It
  // starts again in place (`restarts`), not as another element: the head can move while the caret
  // is in the box with nothing typed (a Refresh pressed before answers; the workspace is asked
  // again by itself when the network returns), and the person goes on typing where they were.
  //
  // What was saved is compared as what the version holds (`sameStored`): rules, none, or a stored
  // file that cannot be read, by its text.
  const atHead = saved.data;
  // The rules of the person's own save that succeeded, until the page shows them as the saved
  // rules of the version shown (the version that save made, or a later one that still holds
  // them). From then on they are what the person starts from. Rules on the page that are still
  // the ones sent are no edits any more; anything else (text typed since, text that cannot be
  // read) stays an edit, of them. Nothing on the page changes by this: what stops being an edit
  // says the same as the saved rules now drawn.
  const [ownSave, setOwnSave] = useState<NamingRules | null>(null);
  if (ownSave !== null && atHead?.kind === 'rules' && sameRules(atHead.rules, ownSave)) {
    setOwnSave(null);
    setBasis(atHead);
    if (edited !== undefined && !unreadable && sameRules(edited, ownSave)) {
      setEdited(undefined);
      setTyped(null);
    }
  }
  /** Counts the times the YAML view started again from rules that changed with nothing edited. */
  const [followed, setFollowed] = useState(0);
  const heldUnread = reading.pending || unreadable;
  if (
    atHead !== undefined &&
    !changed &&
    !heldUnread &&
    (basis === undefined || !sameStored(basis, atHead))
  ) {
    setBasis(atHead);
    if (basis !== undefined && edited === undefined && typed === null) setFollowed(followed + 1);
  }
  const replaced =
    changed && atHead !== undefined && basis !== undefined && !sameStored(basis, atHead);
  // The repair starts in the YAML view, and stays there once the file is repaired.
  if (repairing && view !== 'yaml') setView('yaml');
  const editorHandle = useRef<YamlEditorHandle | null>(null);
  const fileProblems =
    fromFile === undefined ? [] : fromFile.problems.map((each) => placed(each, filePlaces));
  // The rules drawn are those read before, and the rules of the version shown are not on the
  // page (being read, or not readable): saving now would go over rules the page has not shown.
  const notShown = atHead === undefined;
  const notShownId = useId();
  const [replacing, setReplacing] = useState(false);
  const theirsId = useId();
  const keepSavedButton = useRef<HTMLButtonElement | null>(null);
  /** Counts the times the edits were dropped: the YAML view then starts again from the value. */
  const [dropped, setDropped] = useState(0);

  const [said, setSaid] = useState('');
  const [folder, setFolder] = useState('');
  const [folderWrong, setFolderWrong] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [refused, setRefused] = useState<RenamePlan | null>(null);
  const [asking, setAsking] = useState(false);
  // Where focus goes in the commit that shows what an action did.
  const focusNext = useRef<
    | 'heading'
    | 'preview'
    | 'keep'
    | 'apply'
    | 'problems'
    | 'stay'
    | 'opener'
    | 'keepSaved'
    | 'save'
    | null
  >('heading');
  const stayButton = useRef<HTMLButtonElement | null>(null);
  /** What asked to leave: where focus goes back when the person stays. */
  const opener = useRef<HTMLElement | null>(null);
  // The question before a renaming, watched: whether it left the page with keyboard focus in it.
  const heldInQuestion = useRef<HTMLElement | null>(null);
  const [, questionRef] = useWatchedPart<HTMLFieldSetElement>(heldInQuestion);
  useLayoutEffect(() => {
    const left = takeHeld(heldInQuestion);
    const next = focusNext.current;
    const target =
      next === null
        ? null
        : {
            heading: headingRef,
            preview: previewRef,
            keep: keepButton,
            apply: applyButton,
            problems: problemsRef,
            stay: stayButton,
            keepSaved: keepSavedButton,
            save: saveButton,
            // What asked to leave, when it is still on the page; else the panel's heading.
            opener: opener.current?.isConnected === true ? opener : headingRef,
          }[next].current;
    if (target !== null) {
      focusNext.current = null;
      target.focus();
      return;
    }
    // The question left by itself: to the button that opened it, else the panel's heading.
    if (left) (applyButton.current ?? headingRef.current)?.focus();
  });

  const save = useChange<NamingRules, HeadChange>(
    saveRules,
    (result, carried) => {
      setOwnSave(carried);
      setSaid(
        result.adopted
          ? `The rules are saved. The workspace now shows revision ${short(result.revision)}, which includes them.`
          : `The rules are saved, at revision ${short(result.revision)}.`,
      );
    },
    saveButton,
    // A failed save is repeated only while the rules on screen are the ones it carried.
    (carried) => rules !== null && sameRules(carried, rules),
  );
  const previewing = useMutation({
    mutationFn: (asked: { rules: NamingRules; folder: string; head: Revision }) =>
      previewRenames(workspaceId, asked.head, asked.rules, asked.folder),
    onSuccess: (made) => {
      setPreview(made);
      setAsking(false);
      setSaid('The preview is ready. Nothing has been renamed.');
      focusNext.current = 'preview';
    },
  });
  const previewFailure = useRetry({
    request: workspaceId,
    failed: previewing.isError,
    error: previewing.error,
    run: () =>
      previewing.variables === undefined
        ? Promise.resolve()
        : previewing.mutateAsync(previewing.variables),
    loaded: () => previewButton.current,
  });
  // Why the preview on screen can no longer be applied; null while it can.
  const stale = preview === null ? null : staleReason(preview, head, value, refused);
  // A question whose preview went out of date is over: it does not come back by itself.
  if (asking && stale !== null) setAsking(false);
  const apply = useChange<Preview, Renamed>(
    applyPreview,
    (result, applied) => {
      const renamed = applied.plan.entries.filter((entry) => entry.from !== entry.to).length;
      setPreview(null);
      setAsking(false);
      const done =
        `${count(renamed, 'item was', 'items were')} renamed.` +
        (result.adopted
          ? ` The workspace now shows revision ${short(result.revision)}, which includes the change.`
          : ` The renaming made revision ${short(result.revision)}.`);
      setSaid(done);
      // The renaming is done. How the person's drafts of the renamed items stand is said after
      // it, when they have followed, unless something else has been said since.
      void result.drafts.then((outcomes) => {
        const of = (...kinds: string[]) => outcomes.filter((each) => kinds.includes(each)).length;
        const following = of('follows');
        const not = of('asks', 'not-saved');
        const behind = of('behind');
        const drafts =
          (following > 0
            ? ` ${count(following, 'draft of yours follows', 'drafts of yours follow')} the renaming: the next Snapshot will not ask about it.`
            : '') +
          (not > 0
            ? ` ${count(not, 'draft of yours', 'drafts of yours')} could not follow it: the next Snapshot will first ask whether to keep your text.`
            : '') +
          (behind > 0
            ? ` ${count(behind, 'draft of yours is', 'drafts of yours are')} still on the older version this page shows: until you refresh the workspace, saving may be refused, and the next Snapshot will first ask whether to keep your text.`
            : '');
        if (drafts !== '') setSaid((now) => (now === done ? `${done}${drafts}` : now));
      });
      focusNext.current = 'heading';
    },
    previewButton,
    // A failed renaming is repeated only while its preview is the one on screen and can still be
    // applied: a retry never applies a preview the page itself would not apply.
    (carried) => carried === preview && stale === null,
  );
  // The plan last sent. When the service refuses it as a conflict it is out of date, and is
  // never sent again; a later preview is another plan, with its own answer to come.
  const [sent, setSent] = useState<RenamePlan | null>(null);
  if (apply.conflict !== null && sent !== null && refused !== sent) {
    setRefused(sent);
    setAsking(false);
    focusNext.current = 'apply';
  }

  const renames = preview === null ? [] : preview.plan.entries.filter((e) => e.from !== e.to);

  /**
   * Whether the YAML view holds text that is not read yet (typed a moment ago, or being read).
   * Nothing is sent or applied then: the rules on the page are older than the text. Its reading
   * starts at once, and the page says why the press did nothing.
   */
  const notReadYet = () => {
    if (!reading.unread()) return false;
    reading.flush();
    setSaid(NOT_READ_YET);
    return true;
  };
  /** The rules, when they can be sent; otherwise focus goes to the list of what is wrong. */
  const sendable = () => {
    if (notReadYet()) return null;
    if (rules !== null) return rules;
    setSaid('');
    focusNext.current = 'problems';
    problemsRef.current?.focus();
    return null;
  };
  // Leaving with edits that are not saved asks first.
  const [leaving, setLeaving] = useState<{ proceed: () => void; stay?: () => void } | null>(null);
  /** Whether leaving now would lose edits; text that is not read yet counts as one. */
  const unsavedNow = () => reading.unread() || unreadable || changed;
  const leave: LeaveGuard = (proceed, stay) => {
    if (!unsavedNow()) {
      proceed();
      return;
    }
    // A question still open is over: the one who asked it is told that the person stayed.
    leaving?.stay?.();
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    focusNext.current = 'stay';
    setLeaving(stay === undefined ? { proceed } : { proceed, stay });
  };
  const guard = useRef(leave);
  const unsaved = useRef(unsavedNow);
  useEffect(() => {
    guard.current = leave;
    unsaved.current = unsavedNow;
  });
  useEffect(() => {
    if (leaveGuard === undefined) return;
    leaveGuard.current = (proceed, stay) => guard.current(proceed, stay);
    return () => {
      leaveGuard.current = null;
    };
  }, [leaveGuard]);
  // A reload or a closed browser tab: the browser asks, as it does for a draft's text.
  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (unsaved.current()) event.preventDefault();
    };
    window.addEventListener('beforeunload', beforeUnload);
    return () => window.removeEventListener('beforeunload', beforeUnload);
  }, []);
  const startPreview = () => {
    if (previewing.isPending) return;
    const wrong = folderProblem(folder);
    setFolderWrong(wrong);
    if (wrong !== null) {
      folderRef.current?.focus();
      return;
    }
    const asked = sendable();
    if (asked === null) return;
    setSaid('');
    previewing.mutate({ rules: asked, folder, head });
  };
  const startSave = () => {
    // Nothing is saved over rules the page has not shown, and the rules saved now are not saved
    // again. Any rules replace a stored file that cannot be read.
    if (atHead === undefined) return;
    const asked = sendable();
    const now = startingRules(atHead);
    if (asked === null || (now !== undefined && sameRules(asked, now))) return;
    setSaid('');
    save.start(asked);
  };
  /** Saving would replace rules someone else saved: that is asked first. */
  const askReplace = () => {
    if (sendable() === null) return;
    focusNext.current = 'keepSaved';
    setReplacing(true);
  };
  const takeSaved = () => {
    // Text typed a moment ago is dropped with the rest: read later, it would bring the edits back.
    reading.cancel();
    setEdited(undefined);
    setTyped(null);
    setDropped((count) => count + 1);
    setReplacing(false);
    setSaid('These are the saved rules now. Your changes were dropped.');
    focusNext.current = 'heading';
  };

  return (
    <section className="naming-rules" aria-labelledby={heading}>
      <div className="panel-title-row">
        <h2 id={heading} ref={headingRef} tabIndex={-1} className="panel-heading">
          Naming rules
        </h2>
        <button type="button" onClick={() => leave(onClose)}>
          Close the naming rules
        </button>
      </div>
      {leaving !== null && (
        <fieldset className="confirm">
          <legend>Leave the naming rules without saving?</legend>
          <p>
            Your changes to the rules are not saved. Leaving discards them; the saved rules and the
            workspace stay as they are.
          </p>
          <p className="change-buttons">
            <button
              type="button"
              ref={stayButton}
              onClick={() => {
                focusNext.current = 'opener';
                setLeaving(null);
                leaving.stay?.();
              }}
            >
              Keep editing
            </button>{' '}
            <button
              type="button"
              onClick={() => {
                // The one who asked places focus when the rules have left the page.
                setLeaving(null);
                leaving.proceed();
              }}
            >
              Discard my changes
            </button>
          </p>
        </fieldset>
      )}
      <p className="meta">
        How files in this workspace are named.{' '}
        {saved.data === undefined && lastRead !== null ? (
          <>
            These are the rules as they were read at revision{' '}
            <code title={lastRead.at}>{short(lastRead.at)}</code>: the rules of the version shown,
            revision <code title={head}>{short(head)}</code>,{' '}
            {saved.isError ? 'could not be read' : 'are being read'}.
          </>
        ) : (
          <>
            These are the rules of the version shown, revision{' '}
            <code title={head}>{short(head)}</code>.
          </>
        )}{' '}
        Changing them here changes nothing until you save them, and a preview renames nothing.
      </p>
      {stored?.kind === 'none' && (
        <p role="note" className="notice">
          <strong>There are no naming rules yet.</strong> No rules are saved in this version, so
          nothing is renamed by them. Start from the empty set below: add a rule, then save.
        </p>
      )}
      {fromFile !== undefined && (
        <div role="note" className="failure-notice" id={fileNoticeId}>
          <p>
            <strong>The saved rules file cannot be read.</strong>
          </p>
          <p>
            This build cannot read it as naming rules, so no rules apply from it until it is
            repaired; it is not the same as having no rules. Its text is in the YAML view below, and
            what is wrong with it is listed under the text. Fix it there, then save: the rules you
            save replace the file.
          </p>
        </div>
      )}
      {/* Present from the start, so what an action did is announced. */}
      <p role="status" className="meta rules-said">
        {previewing.isPending && !previewFailure.retrying
          ? 'Making the preview…'
          : save.pending
            ? 'Saving the rules…'
            : apply.pending
              ? 'Renaming…'
              : said}
      </p>
      {schemaFailure.shown && (
        <FailureNotice
          context="Reading the form for the naming rules"
          error={schemaFailure.error}
          retry={schemaFailure}
          workspaceId={workspaceId}
        />
      )}
      {savedFailure.shown && (
        <FailureNotice
          context="Reading the naming rules"
          error={savedFailure.error}
          retry={savedFailure}
          workspaceId={workspaceId}
        />
      )}
      {(schema.isPending || saved.isPending) && !schemaFailure.shown && !savedFailure.shown && (
        <p role="status">Reading the naming rules…</p>
      )}
      {form !== null && (value !== undefined || repairing) && (
        <>
          <Tabs.Root
            value={repairing ? 'yaml' : view}
            onValueChange={(next) => {
              if (next !== 'form' && next !== 'yaml') return;
              // The other view opens on what the text says: its reading starts now, and the
              // form waits for it.
              reading.flush();
              setView(next);
            }}
          >
            <Tabs.List aria-label="How the rules are shown" className="tab-list">
              {/* No form over a stored file that cannot be read: it is repaired as text. */}
              {!repairing && (
                <Tabs.Tab value="form" className="tab">
                  Form
                </Tabs.Tab>
              )}
              <Tabs.Tab value="yaml" className="tab">
                YAML
              </Tabs.Tab>
            </Tabs.List>
            {!repairing && (
              <Tabs.Panel value="form">
                {value === undefined ? null : reading.pending ? (
                  // Nothing over rules older than the text: an edit made in a form of them would
                  // be undone, or would undo the text, when the reading answers.
                  <p role="status">Reading the text of the YAML view…</p>
                ) : (
                  <>
                    {unreadable && (
                      <p role="note" className="notice">
                        The YAML view holds text that cannot be read. Below are the last rules that
                        could be. A change made here replaces that text.
                      </p>
                    )}
                    {formHolds(value) ? (
                      <RulesForm
                        schema={form}
                        value={value}
                        onChange={(next) => {
                          // The form is one view of the value: the YAML is written from it again,
                          // its keys where they were.
                          setEdited(keepOrder(value, next));
                          setTyped(null);
                        }}
                      />
                    ) : (
                      <p role="note" className="notice">
                        What the YAML view holds is not a set of rules, so there is no form for it.
                      </p>
                    )}
                  </>
                )}
              </Tabs.Panel>
            )}
            <Tabs.Panel value="yaml">
              <p role="note" className="notice">
                Comments are not kept. The service stores the rules as data, not as text: a comment
                typed here is not saved, and is gone from this text as soon as the form changes the
                rules.
              </p>
              <YamlEditor
                key={dropped}
                restarts={followed}
                label="The naming rules as YAML"
                initialText={
                  typed?.text ?? (value !== undefined ? writeYaml(value) : (fromFile?.text ?? ''))
                }
                describedBy={repairing ? `${fileNoticeId} ${findingsId}` : findingsId}
                onChange={reading.change}
                onBlur={reading.flush}
                marks={fileProblems.flatMap((each) => (each.place === null ? [] : [each.place]))}
                handle={editorHandle}
              />
            </Tabs.Panel>
          </Tabs.Root>
          <div
            ref={problemsRef}
            tabIndex={-1}
            className={problems.length > 0 && !unreadable ? 'failure-notice' : undefined}
          >
            {fromFile !== undefined && (
              <div className="failure-notice">
                <p>
                  <strong>
                    What is wrong with the saved file (
                    {count(fileProblems.length, 'problem', 'problems')}
                    ), as the service read it:
                  </strong>
                </p>
                <ul>
                  {keyed(fileProblems, (each) => each.words).map(({ key, value: each }) => (
                    <li key={key}>
                      {each.words}
                      {each.place !== null && (
                        <>
                          {' '}
                          <button
                            type="button"
                            onClick={() => {
                              if (each.place !== null) editorHandle.current?.show(each.place);
                            }}
                          >
                            Go to line {each.place.line}, column {each.place.column}
                          </button>
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            <YamlFindings id={findingsId} read={typed?.read ?? null} />
            {problems.length > 0 && !unreadable && (
              <div role="alert">
                <p>
                  <strong>
                    These rules have {count(problems.length, 'problem', 'problems')}. Nothing is
                    sent until they are fixed.
                  </strong>
                </p>
                <ul>
                  {keyed(problems, (problem) => problem).map(({ key, value: problem }) => (
                    <li key={key}>{problem}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
          <p className="meta">
            {repairing
              ? changed
                ? 'Your text differs from the saved file.'
                : 'This is the saved file, unchanged.'
              : changed
                ? 'These rules differ from the saved ones.'
                : 'These are the saved rules, unchanged.'}
          </p>
          <div className="change-field">
            <label htmlFor={folderId}>Preview the items in folder</label>
            <input
              id={folderId}
              ref={folderRef}
              type="text"
              value={folder}
              autoComplete="off"
              aria-invalid={folderWrong !== null}
              {...(folderWrong === null ? {} : { 'aria-describedby': folderErrorId })}
              onChange={(event) => {
                setFolder(event.target.value);
                setFolderWrong(null);
              }}
            />
            <p className="meta">Leave empty for the whole workspace.</p>
            {folderWrong !== null && (
              <p id={folderErrorId} className="field-error">
                {folderWrong}
              </p>
            )}
          </div>
          {replaced && atHead !== undefined && (
            <div role="note" className="notice">
              <p>
                <strong>The saved rules are no longer the ones you started from.</strong> Other
                rules were saved while you were editing. Below are the rules as they are saved now;
                yours are above. Saving yours replaces them whole: nothing of the others is merged
                in.
              </p>
              {atHead.kind === 'none' ? (
                <p>No rules are saved now.</p>
              ) : (
                <>
                  <label htmlFor={theirsId}>
                    {atHead.kind === 'rules'
                      ? 'The rules as they are saved now (read-only)'
                      : 'The rules file as it is saved now, which cannot be read (read-only)'}
                  </label>
                  <textarea
                    id={theirsId}
                    className="left-draft"
                    readOnly
                    rows={10}
                    value={atHead.kind === 'rules' ? writeYaml(atHead.rules) : atHead.text}
                  />
                  {atHead.kind === 'unreadable' && (
                    <ul>
                      {keyed(
                        atHead.problems.map((each) => placed(each, NO_PLACES).words),
                        (words) => words,
                      ).map(({ key, value: words }) => (
                        <li key={key}>{words}</li>
                      ))}
                    </ul>
                  )}
                </>
              )}
              <p className="change-buttons">
                <button type="button" onClick={takeSaved}>
                  Take the saved rules, and drop mine
                </button>
              </p>
            </div>
          )}
          {replacing && replaced && (
            <fieldset className="confirm">
              <legend>Replace the saved rules with yours?</legend>
              <p>
                The rules saved now are not the ones you started from. Saving yours replaces them
                whole.
              </p>
              <p className="change-buttons">
                <button
                  type="button"
                  ref={keepSavedButton}
                  onClick={() => {
                    focusNext.current = 'save';
                    setReplacing(false);
                  }}
                >
                  Keep the saved rules
                </button>{' '}
                <button
                  type="button"
                  onClick={() => {
                    focusNext.current = 'save';
                    setReplacing(false);
                    startSave();
                  }}
                >
                  Replace them with mine
                </button>
              </p>
            </fieldset>
          )}
          {canWrite && changed && notShown && (
            <p id={notShownId} className="meta">
              Saving is not available now: the rules of the version shown, revision{' '}
              <code title={head}>{short(head)}</code>,{' '}
              {saved.isError
                ? 'could not be read. The notice above has the way to read them again.'
                : 'are still being read.'}{' '}
              Your changes stay on this page. Once those rules are shown here you can save, or
              choose to replace them if they are not the ones you started from.
            </p>
          )}
          <p className="change-buttons">
            {/* aria-disabled rather than disabled: each keeps focus while it works. */}
            <button
              type="button"
              ref={previewButton}
              aria-disabled={previewing.isPending}
              onClick={startPreview}
            >
              {previewing.isPending ? 'Previewing…' : 'Preview the renames'}
            </button>{' '}
            {canWrite && (
              <button
                type="button"
                ref={saveButton}
                aria-disabled={save.pending || !changed || notShown}
                {...(changed && notShown ? { 'aria-describedby': notShownId } : {})}
                onClick={replaced ? askReplace : startSave}
              >
                {save.pending
                  ? 'Saving…'
                  : replaced
                    ? 'Replace the saved rules with mine…'
                    : 'Save the rules'}
              </button>
            )}
          </p>
        </>
      )}
      <ChangeFailure
        context="Saving the rules"
        change={save}
        sameWords={RULES_REFUSED}
        withdrawnWords="The rules on this page are no longer the ones that were sent, so they are not sent again by themselves. Check them, then press “Save the rules” to save them as they are now."
      />
      {previewFailure.shown && (
        <FailureNotice
          context="Previewing the renames"
          error={previewFailure.error}
          retry={previewFailure}
          workspaceId={workspaceId}
        />
      )}
      {preview !== null && (
        <section className="rename-preview" aria-labelledby={previewHeading}>
          <h3 id={previewHeading} ref={previewRef} tabIndex={-1}>
            Preview: what would be renamed
          </h3>
          <p>
            {preview.plan.considered === 0
              ? 'There is no item to rename'
              : `${count(renames.length, 'item', 'items')} would be renamed, and ${count(preview.plan.unchanged, 'keeps its name', 'keep their names')}, of the ${count(preview.plan.considered, 'item', 'items')} considered`}{' '}
            in{' '}
            {preview.folder === ''
              ? 'the whole workspace'
              : `the folder ${preview.folder} and the folders under it`}{' '}
            at revision{' '}
            <code title={preview.plan.base_revision}>{short(preview.plan.base_revision)}</code>.
            Nothing has been renamed.
          </p>
          <p className="meta">
            {preview.plan.archived_left_out === 0
              ? 'No archived item is among them.'
              : `${count(preview.plan.archived_left_out, 'archived item is', 'archived items are')} left out: archived items are never renamed.`}
          </p>
          {stale !== null && (
            <div role="alert" className="failure-notice">
              <p>
                <strong>This preview is out of date and cannot be applied.</strong>
              </p>
              <p>{stale} Preview again to see what would be renamed now.</p>
            </div>
          )}
          {keyed(preview.plan.warnings, (warning) => JSON.stringify(warning)).map(
            ({ key, value: warning }) => (
              <p key={key} role="note" className="notice">
                {warning.message}
                {warning.location ? ` (${warning.location})` : ''}
              </p>
            ),
          )}
          {renames.length > 0 && (
            // Not a scrolling box of its own: the column scrolls, from any control in it.
            <div className="rename-list">
              <table>
                <caption>The renames, one by one</caption>
                <thead>
                  <tr>
                    <th scope="col">Name now</th>
                    <th scope="col">Name after</th>
                    <th scope="col">Notes</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.plan.entries
                    .filter((entry) => entry.from !== entry.to)
                    .map((entry) => (
                      <tr key={entry.item_id}>
                        <td>{entry.from}</td>
                        <td>{entry.to}</td>
                        <td>{entry.warnings.map((warning) => warning.message).join(' ')}</td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
          )}
          {preview.plan.duplicate_observations.length > 0 && (
            <div role="note" className="notice">
              <p>
                <strong>Paths seen more than once</strong>
              </p>
              <ul>
                {keyed(preview.plan.duplicate_observations, (seen) => seen.path).map(
                  ({ key, value: seen }) => (
                    <li key={key}>
                      {seen.path}: {seen.message} ({count(seen.item_ids.length, 'item', 'items')})
                    </li>
                  ),
                )}
              </ul>
            </div>
          )}
          {canWrite &&
            renames.length > 0 &&
            (asking && stale === null ? (
              <fieldset className="confirm" ref={questionRef}>
                <legend>Rename {count(renames.length, 'item', 'items')}?</legend>
                <p>
                  This is one change of the workspace, made on the version of this preview. Links to
                  the renamed items are rewritten, and the change can be found in the history.
                </p>
                <p className="change-buttons">
                  <button
                    type="button"
                    ref={keepButton}
                    onClick={() => {
                      focusNext.current = 'apply';
                      setAsking(false);
                    }}
                  >
                    Keep the names
                  </button>{' '}
                  <button
                    type="button"
                    aria-disabled={apply.pending}
                    onClick={() => {
                      // The question is on screen only while the preview is the preview of what
                      // the page shows (`stale` above): what is sent is that preview. Text typed
                      // a moment ago is not read yet: nothing is sent, the page says so, and when
                      // the reading finds other rules the preview goes out of date.
                      if (notReadYet()) return;
                      setSent(preview.plan);
                      apply.start(preview);
                    }}
                  >
                    {apply.pending
                      ? 'Renaming…'
                      : `Rename ${count(renames.length, 'item', 'items')}`}
                  </button>
                </p>
              </fieldset>
            ) : (
              <p className="change-buttons">
                <button
                  type="button"
                  ref={applyButton}
                  aria-disabled={stale !== null}
                  onClick={() => {
                    if (stale !== null || notReadYet()) return;
                    focusNext.current = 'keep';
                    setAsking(true);
                  }}
                >
                  Apply this preview…
                </button>
              </p>
            ))}
        </section>
      )}
      <ChangeFailure
        context="Renaming"
        change={apply}
        sameWords={RULES_REFUSED}
        withdrawnWords="That was for a preview that is no longer the one on this page, or that can no longer be applied, so it is not tried again. Preview again, then apply the preview you see."
      />
    </section>
  );
}
