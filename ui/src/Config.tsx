import { useState } from "react";
import { api, type ConfigOption, type Recipe, type Sandbox } from "./api";
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
export function Config(props: { sandbox: Sandbox; recipes: Recipe[]; act: Act; openSoftware: () => void }) {
  const { sandbox, recipes, act } = props;
  const env = sandbox.env ?? {};
  const installed = sandbox.recipes.map((id) => recipes.find((r) => r.id === recipeName(id))).filter((r): r is Recipe => !!r);
  const configurable = installed.filter((recipe) => optionsOf(recipe).length > 0);
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
          {optionsOf(recipe).map((option) => (
            <Field key={option.env} option={option} value={env[option.env] ?? ""} save={(value) => save(option.env, value)} />
          ))}
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
