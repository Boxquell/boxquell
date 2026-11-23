import { KeyRound } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, type SshKeyChoice } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/** Which SSH keys of this computer sandboxes may sign with: for one sandbox, or for all with `null`. */
export function SshKeys(props: { sandbox: string | null; act: Act }) {
  const { sandbox, act } = props;
  const [choice, setChoice] = useState<SshKeyChoice | null>(null);
  const load = useCallback(() => api.sshKeys(sandbox).then(setChoice, () => {}), [sandbox]);
  useEffect(() => {
    load();
  }, [load]);
  if (!choice) return null;

  const save = (agent: boolean, files: string[]) => act(() => api.setSshKeys(sandbox, { agent, files }).then(setChoice));
  const toggle = (path: string) => save(choice.agent, choice.files.includes(path) ? choice.files.filter((file) => file !== path) : [...choice.files, path]);
  const known = choice.available.keys;
  const others = choice.files.filter((file) => !known.some((key) => key.path === file));
  const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
  const inherited = sandbox ? [...(choice.global.agent ? ["the SSH agent"] : []), ...choice.global.files.map(fileName)] : [];

  return (
    <section>
      <h3>
        <KeyRound size={16} aria-hidden /> SSH keys for git
      </h3>
      <p className="muted">
        {sandbox ? "This sandbox" : "Every sandbox"} can sign in with the keys you switch on here, for example to push to GitHub or GitLab over SSH. The
        keys stay on this computer: the sandbox can ask for a signature but cannot read or copy a key.
      </p>
      {!choice.available.supported && <p className="warning">SSH keys for sandboxes are not available on Windows yet.</p>}
      {/* A sandbox shows its warnings at the top of its page already. */}
      {!sandbox && choice.problems.length > 0 && choice.available.supported && (
        <div className="warning">
          {choice.problems.map((problem) => (
            <span key={problem}>{problem}</span>
          ))}
        </div>
      )}
      <label className="check setting">
        <input type="checkbox" className="switch" checked={choice.agent} onChange={() => save(!choice.agent, choice.files)} />
        <span>
          Everything in my SSH agent
          <small className="muted block">
            {choice.available.agent
              ? "All keys loaded in this computer's SSH agent, including ones with a passphrase or on a security key."
              : "No SSH agent is running on this computer. You only need one for keys with a passphrase; a key without one can be switched on below."}
          </small>
        </span>
      </label>
      {known.map((key) => (
        <label key={key.path} className="check setting">
          <input type="checkbox" className="switch" checked={choice.files.includes(key.path)} onChange={() => toggle(key.path)} />
          <span>
            {fileName(key.path)}
            <small className="muted block">
              {[key.kind, key.comment, key.passphrase ? "has a passphrase: works through the SSH agent only" : ""].filter(Boolean).join(" · ")}
            </small>
          </span>
        </label>
      ))}
      {others.length > 0 && (
        <ul className="rules">
          {others.map((file) => (
            <li key={file}>
              <code>{file}</code>
              <button className="link" onClick={() => toggle(file)}>
                remove
              </button>
            </li>
          ))}
        </ul>
      )}
      {known.length === 0 && <p className="muted">No private keys were found in ~/.ssh.</p>}
      {inherited.length > 0 && <p className="muted">Already shared with all sandboxes: {inherited.join(", ")}.</p>}
      <p className="muted">
        Applies at once, also to running sandboxes. Git over SSH needs the server's SSH port allowed in the network rules, for example{" "}
        <code>github.com:22</code>.
      </p>
    </section>
  );
}
