/**
 * The naming rules of the workspace: a form over them and an advanced YAML view of the same rules,
 * saving them, a preview of what they would rename, and applying a preview the person has seen
 * (SPEC section 6).
 *
 * One value is edited, starting from the saved rules of the version the workspace shows. The form
 * and the YAML view are two views of it, one on screen at a time: a change in the form is written
 * into the YAML, and YAML that can be read becomes what the form shows. The value is kept as it
 * is: a key neither view knows is never dropped. YAML that cannot be read changes nothing: the
 * value stays the last one that could, the text stays as typed (through a change of view too),
 * and its problems are listed with their lines. Nothing is sent while the YAML cannot be read or
 * the value has a problem by the service's schema: the problems are shown at their fields and in
 * a list, and Save and Preview say so instead of asking the service.
 *
 * A preview writes nothing. It lists each item that would be renamed with its name now and its
 * name after, as a table (read row by row with a screen reader's table keys; the column it is in
 * scrolls from any control), with the service's warnings and the duplicate paths it observed. A preview belongs to the revision and the rules it was made of.
 * When either is no longer what the page shows (the person saved the rules, made another change
 * or refreshed; the rules were edited; the service refused the preview as out of date), it is
 * marked out of date and cannot be applied: the person previews again. Applying asks first, and
 * sends back exactly the plan and the rules of the preview on screen. A renaming that failed is
 * tried again by its notice only while its preview is still the one on screen and can still be
 * applied; otherwise the notice offers no retry and says to preview again (SPEC section 6: "A
 * stale preview must not silently apply to changed source state"). A save that failed is tried
 * again only while the rules on screen are the ones it carried.
 *
 * Focus: the panel opens on its heading; a preview puts focus on the preview's heading; the
 * question before renaming opens on the choice that keeps the names, and gives focus back to the
 * button that opened it; after the renaming focus is on the panel's heading. Buttons that can
 * become unavailable with focus use `aria-disabled`.
 */

import { Tabs } from '@base-ui/react/tabs';
import type { RJSFSchema } from '@rjsf/utils';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useId, useLayoutEffect, useRef, useState } from 'react';
import { z } from 'zod';
import type { NamingRules, RenamePlan, Revision } from '../../api/generated/types.gen';
import { keyed } from '../../lib/keys';
import { YamlEditor, YamlFindings } from '../../lib/YamlEditor';
import { readYaml, writeYaml, type YamlRead } from '../../lib/yaml';
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
  NothingToPreview,
  type Preview,
  previewRenames,
  rulesOptions,
  rulesSchemaOptions,
  sameRules,
  saveRules,
} from './rules';

export interface NamingRulesProps {
  /** Leave the naming rules; the caller puts focus back on what opened them. */
  onClose: () => void;
}

const zSchema = z.record(z.string(), z.unknown());
const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const RULES_REFUSED =
  'The service refused it because it does not fit the workspace as it is now. Preview again, then check the rules.';

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

export function NamingRulesPanel({ onClose }: NamingRulesProps) {
  const client = useQueryClient();
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
  // While the rules are read again at a new head, the ones read before stay on screen.
  const saved = useQuery({
    ...rulesOptions(workspaceId, head),
    placeholderData: (previous) => previous,
  });
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
  const value = edited === undefined ? saved.data : edited;
  // Which view is on screen, and the YAML as typed: null while it is the value written out.
  const [view, setView] = useState<'form' | 'yaml'>('form');
  const [typed, setTyped] = useState<{ text: string; read: YamlRead } | null>(null);
  const unreadable = typed !== null && !typed.read.ok;
  const findingsId = useId();
  const shape = zSchema.safeParse(schema.data);
  const form: RJSFSchema | null = shape.success ? shape.data : null;
  const problems = form === null || value === undefined ? [] : rulesProblems(form, value);
  // Without a problem by the service's own schema, the value is rules that can be sent.
  const rules =
    form !== null && !unreadable && problems.length === 0 && formHolds(value) ? value : null;
  const changed =
    edited !== undefined && saved.data !== undefined && !sameRules(edited, saved.data);

  const [said, setSaid] = useState('');
  const [folder, setFolder] = useState('');
  const [folderWrong, setFolderWrong] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [refused, setRefused] = useState<RenamePlan | null>(null);
  const [asking, setAsking] = useState(false);
  // Where focus goes in the commit that shows what an action did.
  const focusNext = useRef<'heading' | 'preview' | 'keep' | 'apply' | 'problems' | null>('heading');
  useLayoutEffect(() => {
    const next = focusNext.current;
    if (next === null) return;
    const target = {
      heading: headingRef,
      preview: previewRef,
      keep: keepButton,
      apply: applyButton,
      problems: problemsRef,
    }[next].current;
    if (target === null) return;
    focusNext.current = null;
    target.focus();
  });

  const save = useChange<NamingRules, HeadChange>(
    saveRules,
    (result) =>
      setSaid(
        `The rules are saved. The workspace now shows revision ${short(result.revision)}, which includes them.`,
      ),
    saveButton,
    // A failed save is repeated only while the rules on screen are the ones it carried.
    (carried) => rules !== null && sameRules(carried, rules),
  );
  const previewing = useMutation({
    mutationFn: (asked: { rules: NamingRules; folder: string; head: Revision }) =>
      previewRenames(client, workspaceId, asked.head, asked.rules, asked.folder),
    onSuccess: (made) => {
      setPreview(made);
      setAsking(false);
      setSaid('The preview is ready. Nothing has been renamed.');
      focusNext.current = 'preview';
    },
  });
  const nothing = previewing.isError && previewing.error instanceof NothingToPreview;
  const previewFailure = useRetry({
    request: workspaceId,
    failed: previewing.isError && !nothing,
    error: previewing.error,
    run: () =>
      previewing.variables === undefined
        ? Promise.resolve()
        : previewing.mutateAsync(previewing.variables),
    loaded: () => previewButton.current,
  });
  // Why the preview on screen can no longer be applied; null while it can.
  const stale = preview === null ? null : staleReason(preview, head, value, refused);
  const apply = useChange<Preview, HeadChange>(
    applyPreview,
    (result, applied) => {
      const renamed = applied.plan.entries.filter((entry) => entry.from !== entry.to).length;
      setPreview(null);
      setAsking(false);
      setSaid(
        `${count(renamed, 'item was', 'items were')} renamed. The workspace now shows revision ${short(result.revision)}, which includes the change.`,
      );
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
  const kept = preview === null ? 0 : preview.plan.entries.length - renames.length;

  /** The rules, when they can be sent; otherwise focus goes to the list of what is wrong. */
  const sendable = () => {
    if (rules !== null) return rules;
    setSaid('');
    focusNext.current = 'problems';
    problemsRef.current?.focus();
    return null;
  };
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
    const asked = sendable();
    if (asked === null || !changed) return;
    setSaid('');
    save.start(asked);
  };

  return (
    <section className="naming-rules" aria-labelledby={heading}>
      <div className="panel-title-row">
        <h2 id={heading} ref={headingRef} tabIndex={-1} className="panel-heading">
          Naming rules
        </h2>
        <button type="button" onClick={onClose}>
          Close the naming rules
        </button>
      </div>
      <p className="meta">
        How files in this workspace are named. These are the rules of the version shown, revision{' '}
        <code title={head}>{short(head)}</code>. Changing them here changes nothing until you save
        them, and a preview renames nothing.
      </p>
      {/* Present from the start, so what an action did is announced. */}
      <p role="status" className="meta rules-said">
        {previewing.isPending && !previewFailure.retrying
          ? 'Listing the items and making the preview…'
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
      {form !== null && value !== undefined && (
        <>
          <Tabs.Root
            value={view}
            onValueChange={(next) => {
              if (next === 'form' || next === 'yaml') setView(next);
            }}
          >
            <Tabs.List aria-label="How the rules are shown" className="tab-list">
              <Tabs.Tab value="form" className="tab">
                Form
              </Tabs.Tab>
              <Tabs.Tab value="yaml" className="tab">
                YAML
              </Tabs.Tab>
            </Tabs.List>
            <Tabs.Panel value="form">
              {unreadable && (
                <p role="note" className="notice">
                  The YAML view holds text that cannot be read. Below are the last rules that could
                  be. A change made here replaces that text.
                </p>
              )}
              {formHolds(value) ? (
                <RulesForm
                  schema={form}
                  value={value}
                  onChange={(next) => {
                    // The form is one view of the value: the YAML is written from it again.
                    setEdited(next);
                    setTyped(null);
                  }}
                />
              ) : (
                <p role="note" className="notice">
                  What the YAML view holds is not a set of rules, so there is no form for it.
                </p>
              )}
            </Tabs.Panel>
            <Tabs.Panel value="yaml">
              <YamlEditor
                label="The naming rules as YAML"
                initialText={typed?.text ?? writeYaml(value)}
                describedBy={findingsId}
                onChange={(text) => {
                  const read = readYaml(text);
                  setTyped({ text, read });
                  // Only YAML that can be read becomes the rules; otherwise they stay as they were.
                  if (read.ok) setEdited(read.value);
                }}
              />
            </Tabs.Panel>
          </Tabs.Root>
          <div
            ref={problemsRef}
            tabIndex={-1}
            className={problems.length > 0 && !unreadable ? 'failure-notice' : undefined}
          >
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
            {changed
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
                aria-disabled={save.pending || !changed}
                onClick={startSave}
              >
                {save.pending ? 'Saving…' : 'Save the rules'}
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
      {nothing && (
        <div role="alert" className="failure-notice">
          <p>
            <strong>There is nothing to preview.</strong>
          </p>
          <p>
            {folder === '' ? 'This workspace' : `The folder ${folder}`} holds no item, so nothing
            was asked of the service.
          </p>
        </div>
      )}
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
            {count(renames.length, 'item', 'items')} would be renamed
            {kept > 0 ? `, and ${count(kept, 'keeps its name', 'keep their names')}` : ''}, of the{' '}
            {count(preview.looked, 'item', 'items')} in{' '}
            {preview.folder === '' ? 'the whole workspace' : `the folder ${preview.folder}`} at
            revision{' '}
            <code title={preview.plan.base_revision}>{short(preview.plan.base_revision)}</code>.
            Nothing has been renamed.
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
              <fieldset className="confirm">
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
                      // the page shows (`stale` above): what is sent is that preview.
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
                    if (stale !== null) return;
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
