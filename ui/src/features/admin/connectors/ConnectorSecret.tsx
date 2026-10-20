/**
 * A new connector's secret, shown this once (ruling C5; SPEC section 11: "A newly issued connector
 * secret is returned exactly once and never stored in plain text").
 *
 * It is shown from the state of the component that issued it, which the caller clears when the
 * person hides it, and which goes with the page: after a reload or a visit elsewhere it is not
 * there, and nothing can show it again (the service keeps only its hash). The words say so, and how
 * to put it in an MCP client: the CLI's stdio relay (`okf-jawn mcp --stdio`) reads it from the
 * `OKF_JAWN_TOKEN` environment variable or a file named by `--token-file`, and a client that
 * speaks HTTP sends it as a bearer token to this service's `/mcp` endpoint
 * (`crates/cli/src/lib.rs`, `crates/server/src/identity.rs`).
 *
 * It takes keyboard focus when it appears (`tabindex="-1"`, from script only), so its words are
 * read first. The field selects the whole secret when it takes focus, to be copied.
 */
import { type RefObject, useId } from 'react';
import type { IssuedConnector } from '../../../api/generated/types.gen';
import { nameOf } from './connectors';

export interface ConnectorSecretProps {
  issued: IssuedConnector;
  /** The group, which takes focus when it appears. */
  boxRef: RefObject<HTMLFieldSetElement | null>;
  /** Hide the secret: the person has put it in their client. */
  onHide: () => void;
}

export function ConnectorSecret({ issued, boxRef, onHide }: ConnectorSecretProps) {
  const fieldId = useId();
  const origin = typeof window === 'undefined' ? '' : window.location.origin;
  return (
    <fieldset
      ref={boxRef}
      tabIndex={-1}
      className="my-2 grid justify-items-start gap-1 rounded border-2 border-[color:CanvasText] px-3 py-2"
    >
      <legend className="px-1 font-bold">The secret of “{nameOf(issued.connector)}”</legend>
      <p className="m-0">
        It is shown only this once: it will not be shown again, here or anywhere. The service keeps
        only a fingerprint of it, so nobody can show it to you later. If it is lost, revoke this
        connector and issue a new one.
      </p>
      <label htmlFor={fieldId} className="font-bold">
        Secret
      </label>
      <input
        id={fieldId}
        type="text"
        readOnly
        value={issued.secret}
        autoComplete="off"
        spellCheck={false}
        className="w-[min(100%,36rem)] font-mono"
        onFocus={(event) => event.currentTarget.select()}
      />
      <p className="m-0">Put it in your MCP client now:</p>
      <ul className="m-0 list-disc pl-6">
        <li>
          a client that starts a program runs <code>okf-jawn mcp --stdio</code> with the environment
          variable <code>OKF_JAWN_TOKEN</code> set to this secret (or with the secret as the first
          line of a file, and <code>--token-file</code> followed by that file’s path);
        </li>
        <li>
          a client that connects over HTTP uses the address <code>{origin}/mcp</code> and sends the
          header <code>Authorization: Bearer</code> followed by this secret.
        </li>
      </ul>
      <p className="m-0">
        <button type="button" onClick={onHide}>
          Hide the secret
        </button>
      </p>
    </fieldset>
  );
}
