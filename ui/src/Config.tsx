import { useEffect, useState, type FormEvent } from "react";
import { ExternalLink } from "lucide-react";
import { api, type ConfigOption, type Credential, type Recipe, type RecipeAction, type Sandbox, type Secret } from "./api";
import { BrandIcon, brandColors } from "./BrandIcon";
import { recipeName } from "./RecipeCards";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/** A recipe's settings: what it declares, plus the model variable of an agent as a field of its own. */
function optionsOf(recipe: Recipe): ConfigOption[] {
  const declared = recipe.config ?? [];
  if (!recipe.model_env || declared.some((option) => option.env === recipe.model_env)) return declared;
  return [{ env: recipe.model_env, label: "Model", help: "The model it starts with.", placeholder: "its own default" }, ...declared];
}

/** Settings of the installed software, each one an environment variable of the sandbox, and the variables set by hand. */
export function Config(props: {
  sandbox: Sandbox;
  recipes: Recipe[];
  secrets: Secret[];
  act: Act;
  openSoftware: () => void;
  openTerminal: (sandbox: string, title: string, command?: string) => void;
}) {
  const { sandbox, recipes, act } = props;
  const env = sandbox.env ?? {};
  const installed = sandbox.recipes.map((id) => recipes.find((r) => r.id === recipeName(id))).filter((r): r is Recipe => !!r);
  const configurable = installed.filter((recipe) => optionsOf(recipe).length > 0 || recipe.secrets.length > 0 || (recipe.actions?.length ?? 0) > 0);
  const known = new Set(configurable.flatMap((recipe) => optionsOf(recipe).map((option) => option.env)));
  const others = Object.entries(env).filter(([key]) => !known.has(key));
  const save = (key: string, value: string) => act(() => api.resize(sandbox.name, { env: { [key]: value } }));

  return (
    <>
      {configurable.length === 0 && (
        <section className="centered">
          <h3>Nothing to configure yet</h3>
          <p className="muted">Software with settings shows up here once it is installed.</p>
          <div className="actions">
            <button onClick={props.openSoftware}>Open Software</button>
          </div>
        </section>
      )}
      {configurable.map((recipe) => (
        <section key={recipe.id} className="config">
          <h3 className="config-head">
            <span className={`avatar small ${recipe.icon ? "brand" : ""}`} style={recipe.icon ? brandColors(recipe.icon_color) : { background: "var(--accent)" }} aria-hidden="true">
              <BrandIcon svg={recipe.icon} size={16}>
                {recipe.name.slice(0, 2)}
              </BrandIcon>
            </span>
            {recipe.name}
          </h3>
          {recipe.secrets.length > 0 && (
            <div className="credentials">
              {recipe.secrets.length > 1 && <p className="muted">It signs in with one of these; one is enough.</p>}
              {recipe.secrets.map((name) => (
                <CredentialField
                  key={name}
                  sandbox={sandbox}
                  credential={recipe.credentials?.find((credential) => credential.name === name) ?? { name }}
                  shared={props.secrets.some((secret) => secret.name === name)}
                  act={act}
                />
              ))}
            </div>
          )}
          {(recipe.actions ?? []).map((action, index) => (
            <ActionRow key={action.label} sandbox={sandbox.name} recipe={recipe.id} index={index} action={action} openTerminal={props.openTerminal} />
          ))}
          {optionsOf(recipe).map((option) =>
            option.kind === "port" ? (
              <PortField key={option.env} sandbox={sandbox} recipe={recipe.id} option={option} value={env[option.env] ?? ""} act={act} />
            ) : (
              <Field key={option.env} option={option} value={env[option.env] ?? ""} save={(value) => save(option.env, value)} />
            ),
          )}
        </section>
      ))}
      <section>
        <h3>Other variables</h3>
        <p className="muted">Environment variables for every session in this sandbox. Changes reach a running sandbox at once and apply to sessions opened from then on.</p>
        {others.length > 0 && (
          <ul className="rules">
            {others.map(([key, value]) => (
              <li key={key}>
                <code>
                  {key}={value}
                </code>
                <button className="link" onClick={() => save(key, "")}>
                  remove
                </button>
              </li>
            ))}
          </ul>
        )}
        <NewVariable save={save} />
      </section>
    </>
  );
}

/**
 * A key the software signs in with. Its value is stored on this computer for
 * this sandbox; the sandbox itself only ever holds a placeholder.
 */
function CredentialField(props: { sandbox: Sandbox; credential: Credential; shared: boolean; act: Act }) {
  const { sandbox, credential, act } = props;
  const [value, setValue] = useState("");
  const [changing, setChanging] = useState(false);
  const own = sandbox.local_secrets.some((secret) => secret.name === credential.name);
  const attached = own || sandbox.secrets.includes(credential.name);
  const about = (
    <span>
      {credential.label || credential.name}
      <small className="muted block">
        {attached ? (own ? "Stored for this sandbox. " : "Uses the shared secret. ") : ""}
        {credential.help}{" "}
        {credential.url && (
          <a href={credential.url} target="_blank" rel="noreferrer">
            Get one <ExternalLink size={12} aria-hidden="true" />
          </a>
        )}
      </small>
    </span>
  );
  if (attached && !changing) {
    return (
      <div className="setting field">
        <span className="row tight">
          <span className="chip on">in use</span>
          <button className="link" onClick={() => setChanging(true)}>
            change
          </button>
          {own && (
            <button className="link bad" onClick={() => act(() => api.removeLocalSecret(sandbox.name, credential.name))}>
              remove
            </button>
          )}
        </span>
        {about}
      </div>
    );
  }
  return (
    <form
      className="setting field"
      onSubmit={(event) => {
        event.preventDefault();
        act(() => api.setLocalSecret(sandbox.name, credential.name, value, credential.hosts?.length ? credential.hosts : undefined, credential.env?.length ? credential.env : undefined)).then(() => {
          setValue("");
          setChanging(false);
        });
      }}
    >
      <span className="row tight">
        <input type="password" required autoComplete="off" placeholder="paste the key" value={value} onChange={(e) => setValue(e.target.value)} aria-label={credential.label || credential.name} />
        <button className="primary">Save</button>
        {props.shared && !attached && (
          <button type="button" onClick={() => act(() => api.attachSecret(sandbox.name, credential.name))} title="A secret of this name is stored for all sandboxes">
            Use shared
          </button>
        )}
      </span>
      {about}
    </form>
  );
}

/** Quote a value for the shell, as the daemon does for commands it runs itself. */
function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** A command of the software as a button, with fields for the values it needs and its output below. */
function ActionRow(props: { sandbox: string; recipe: string; index: number; action: RecipeAction; openTerminal: (sandbox: string, title: string, command?: string) => void }) {
  const { action } = props;
  const inputs = action.inputs ?? [];
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; output: string } | null>(null);
  const run = () => {
    if (action.terminal) {
      // Questions are answered in a terminal; the values typed here are put in first.
      const command = inputs.reduce((text, input) => text.replaceAll(`{${input.name}}`, shellQuote(values[input.name]?.trim() || input.default || "")), action.command);
      props.openTerminal(props.sandbox, action.label, command);
      return;
    }
    setBusy(true);
    setResult(null);
    api
      .runAction(props.sandbox, props.recipe, props.index, values)
      .then(setResult, (err) => setResult({ ok: false, output: String(err.message ?? err) }))
      .finally(() => setBusy(false));
  };
  return (
    <form
      className="action"
      onSubmit={(event) => {
        event.preventDefault();
        run();
      }}
    >
      <div className="setting field">
        <span className="row tight">
          <button className="primary" disabled={busy}>
            {busy ? "Working…" : action.label}
          </button>
        </span>
        <span>
          {action.label}
          <small className="muted block">
            {action.help} {action.terminal ? "Opens in a terminal." : ""}
          </small>
        </span>
      </div>
      {inputs.length > 0 && (
        <div className="row action-inputs">
          {inputs.map((input) => (
            <label key={input.name} className="grow">
              {input.label}
              <input
                required={input.default == null}
                spellCheck={false}
                placeholder={input.placeholder || input.default || ""}
                value={values[input.name] ?? ""}
                onChange={(e) => setValues((all) => ({ ...all, [input.name]: e.target.value }))}
                title={input.help}
              />
            </label>
          ))}
        </div>
      )}
      {result && (
        <pre className={`action-output ${result.ok ? "" : "failed"}`} role="status">
          {result.output || (result.ok ? "Done." : "It ended with an error and printed nothing.")}
        </pre>
      )}
    </form>
  );
}

function PortField(props: { sandbox: Sandbox; recipe: string; option: ConfigOption; value: string; act: Act }) {
  const { sandbox, recipe, option, value, act } = props;
  const [draft, setDraft] = useState(value);
  const [error, setError] = useState("");
  useEffect(() => setDraft(value), [value]);
  const canChange = !sandbox.running || value === "";
  const update = (port?: number, random = false) =>
    act(() => api.setRecipePort(sandbox.name, recipe, port, random)).then(() => setError(""));
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const port = Number(draft);
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      setError("Enter a free port from 1024 to 65535.");
      return;
    }
    update(port);
  };
  const mapped = Number(value);
  const forward = sandbox.ports.find((port) => port.host === mapped && port.guest === mapped);
  return (
    <>
      <form className="setting field" onSubmit={submit}>
        <span className="row tight">
          <input
            type="number"
            min="1024"
            max="65535"
            required
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            aria-label={option.label}
            disabled={!canChange}
          />
          <button type="submit" disabled={!canChange}>Set</button>
          <button type="button" onClick={() => update(undefined, true)} disabled={!canChange}>Random</button>
        </span>
        <span>
          {option.label}
          <small className="muted block">
            {option.help} <code className="faint">{option.env}</code>
          </small>
          {forward && <small className="muted block">Host forward: 127.0.0.1:{forward.host} → guest {forward.guest}</small>}
          {sandbox.running && value && <small className="muted block">Stop and start the sandbox before changing its active listener port.</small>}
        </span>
      </form>
      {error && <p className="warning" role="alert">{error}</p>}
    </>
  );
}

/** One setting: a switch, a choice or a field that saves when it is left or Enter is pressed. */
function Field(props: { option: ConfigOption; value: string; save: (value: string) => Promise<void> }) {
  const { option, value } = props;
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? value;
  const commit = () => {
    if (draft !== null && draft.trim() !== value) props.save(draft.trim()).then(() => setDraft(null));
    else setDraft(null);
  };
  const about = (
    <span>
      {option.label}
      <small className="muted block">
        {option.help} <code className="faint">{option.env}</code>
      </small>
    </span>
  );
  if (option.kind === "switch") {
    const on = option.on || "1";
    return (
      <label className="check setting">
        <input type="checkbox" className="switch" checked={value !== ""} onChange={() => props.save(value === "" ? on : "")} />
        {about}
      </label>
    );
  }
  if (option.kind === "choice") {
    return (
      <label className="setting field">
        <select value={value} onChange={(e) => props.save(e.target.value)}>
          <option value="">{option.placeholder || "default"}</option>
          {(option.options ?? []).map((choice) => (
            <option key={choice} value={choice}>
              {choice}
            </option>
          ))}
          {value !== "" && !(option.options ?? []).includes(value) && <option value={value}>{value}</option>}
        </select>
        {about}
      </label>
    );
  }
  const list = `options-${option.env}`;
  return (
    <label className="setting field">
      <input
        type={option.kind === "number" ? "number" : "text"}
        spellCheck={false}
        list={option.options?.length ? list : undefined}
        placeholder={option.placeholder || "not set"}
        value={shown}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
      {option.options?.length ? (
        <datalist id={list}>
          {option.options.map((choice) => (
            <option key={choice} value={choice} />
          ))}
        </datalist>
      ) : null}
      {about}
    </label>
  );
}

function NewVariable(props: { save: (key: string, value: string) => Promise<void> }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  return (
    <form
      className="row"
      onSubmit={(event) => {
        event.preventDefault();
        props.save(name.trim(), value).then(() => {
          setName("");
          setValue("");
        });
      }}
    >
      <label>
        Name
        <input required pattern="[A-Za-z_][A-Za-z0-9_]*" placeholder="MY_VARIABLE" spellCheck={false} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="grow">
        Value
        <input required placeholder="value" spellCheck={false} value={value} onChange={(e) => setValue(e.target.value)} />
      </label>
      <button>Set</button>
    </form>
  );
}
