import type { Recipe } from "./api";

const TITLES: Record<Recipe["category"], string> = { agent: "Agents", language: "Languages", tool: "Tools", ide: "IDEs" };

/** `node@22` → `node`. */
export const recipeName = (entry: string) => entry.split("@")[0];

/**
 * Checkboxes for recipes. `installed` and `picked` hold entries such as
 * `claude` or `node@22`; a recipe with versions gets a version menu.
 */
export function RecipePicker(props: { recipes: Recipe[]; installed: string[]; picked: string[]; onChange: (ids: string[]) => void }) {
  const without = (id: string) => props.picked.filter((p) => recipeName(p) !== id);
  return (
    <div className="picker">
      {(Object.keys(TITLES) as Recipe["category"][]).map((category) => {
        const list = props.recipes.filter((r) => r.category === category);
        if (list.length === 0) return null;
        return (
          <fieldset key={category}>
            <legend>{TITLES[category]}</legend>
            {list.map((r) => {
              const installed = props.installed.find((entry) => recipeName(entry) === r.id);
              const picked = props.picked.find((entry) => recipeName(entry) === r.id);
              const version = (picked ?? installed)?.split("@")[1] ?? r.versions[0];
              const entry = (v: string | undefined) => (v ? `${r.id}@${v}` : r.id);
              // Something installed can only be picked again in another version.
              const locked = !!installed && !picked;
              return (
                <div key={r.id} className="check" title={describe(r)}>
                  <label className="check">
                    <input
                      type="checkbox"
                      disabled={locked}
                      checked={!!installed || !!picked}
                      onChange={() => props.onChange(picked ? without(r.id) : [...props.picked, entry(version)])}
                    />
                    {r.name}
                  </label>
                  {r.versions.length > 0 && (
                    <select
                      className="version"
                      aria-label={`${r.name} version`}
                      value={version}
                      onChange={(e) => {
                        const chosen = entry(e.target.value);
                        props.onChange(chosen === installed ? without(r.id) : [...without(r.id), chosen]);
                      }}
                    >
                      {r.versions.map((v) => (
                        <option key={v}>{v}</option>
                      ))}
                    </select>
                  )}
                  {r.source !== "builtin" && <small className="muted">{r.source}</small>}
                </div>
              );
            })}
          </fieldset>
        );
      })}
    </div>
  );
}

/** Tooltip: what it is and, for recipes from elsewhere, what they open. */
function describe(r: Recipe): string {
  const hosts = [...r.allow, ...r.install_allow];
  if (r.source === "builtin" || hosts.length === 0) return r.description;
  return `${r.description}\nFrom ${r.source}. Opens: ${hosts.join(", ")}`;
}
