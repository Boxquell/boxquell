import { Search } from "lucide-react";
import { useState } from "react";
import { BrandIcon, brandColors } from "./BrandIcon";
import type { Recipe } from "./api";

const TITLES: Record<Recipe["category"], [string, string]> = {
  agent: ["Agents", "Coding agents that work in this sandbox"],
  language: ["Languages", "Runtimes and toolchains, with a choice of version"],
  tool: ["Tools", "Command-line tools and build helpers"],
  ide: ["IDEs", "Editors that run in the sandbox"],
};

/** `languages:node@22` → `languages:node`. */
export const recipeName = (entry: string) => entry.split("@")[0];

/** A stable colour per recipe, so each card is recognisable at a glance. */
function hue(id: string): number {
  let hash = 0;
  for (const char of id) hash = (hash * 31 + char.charCodeAt(0)) % 360;
  return hash;
}

/**
 * The software catalogue as cards. `installed` and `picked` hold entries such
 * as `agents:claude` or `languages:node@22`.
 */
export function RecipeCards(props: {
  recipes: Recipe[];
  installed: string[];
  picked: string[];
  busy: boolean;
  onChange: (ids: string[]) => void;
  /** Installed software for which there is something newer, by recipe id */
  updates?: { id: string; to: string; note: string }[];
  onUpdate?: (entry: string) => void;
  onRemove?: (id: string) => void;
}) {
  const without = (id: string) => props.picked.filter((p) => recipeName(p) !== id);
  const [query, setQuery] = useState("");
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (r: Recipe) => words.every((word) => `${r.name} ${r.id} ${r.description} ${r.category}`.toLowerCase().includes(word));
  const found = props.recipes.filter(matches);
  return (
    <>
      <label className="search wide">
        <Search size={15} aria-hidden />
        <input type="search" placeholder="Search agents, languages and tools…" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search the software" />
      </label>
      {found.length === 0 && <p className="muted">Nothing matches “{query}”. More software comes from repositories, under Settings › Plugins.</p>}
      {(Object.keys(TITLES) as Recipe["category"][]).map((category) => {
        const list = found.filter((r) => r.category === category);
        if (list.length === 0) return null;
        const [title, about] = TITLES[category];
        return (
          <div key={category} className="shelf">
            <h3>
              {title} <span className="muted plain">{about}</span>
            </h3>
            <div className="cards">
              {list.map((r) => {
                const installed = props.installed.find((entry) => recipeName(entry) === r.id);
                const picked = props.picked.find((entry) => recipeName(entry) === r.id);
                const version = (picked ?? installed)?.split("@")[1] ?? r.versions[0];
                const entry = (v: string | undefined) => (v ? `${r.id}@${v}` : r.id);
                const state = picked ? "picked" : installed ? "installed" : "";
                const update = installed ? props.updates?.find((candidate) => candidate.id === r.id) : undefined;
                const toggle = () => !props.busy && (!installed || picked) && props.onChange(picked ? without(r.id) : [...props.picked, entry(version)]);
                return (
                  <div key={r.id} className={`card ${state}`}>
                    <button type="button" className="card-main" onClick={toggle} aria-pressed={!!picked} disabled={props.busy || (!!installed && !picked)}>
                      <span className={`avatar ${r.icon ? "brand" : ""}`} style={r.icon ? brandColors(r.icon_color) : { background: `hsl(${hue(r.id)} 55% 42%)` }} aria-hidden="true">
                        <BrandIcon svg={r.icon}>{r.name.replace(/[^A-Za-z0-9+#]/g, "").slice(0, 2)}</BrandIcon>
                      </span>
                      <span className="grow">
                        <strong>{r.name}</strong>
                        <small>{r.description}</small>
                      </span>
                    </button>
                    <div className="card-foot">
                      {installed ? (
                        <>
                          <span className="chip on">installed{installed.includes("@") ? ` ${installed.split("@")[1]}` : ""}</span>
                          {update && props.onUpdate && (
                            <button type="button" className="link" disabled={props.busy} title={update.note} onClick={() => props.onUpdate!(update.to)}>
                              Update
                            </button>
                          )}
                          {props.onRemove && (
                            <button type="button" className="link bad" disabled={props.busy} title={`Remove ${r.name} from this sandbox`} onClick={() => props.onRemove!(r.id)}>
                              Remove
                            </button>
                          )}
                        </>
                      ) : (
                        <span className="chip">{picked ? "selected" : r.official ? "official" : r.source}</span>
                      )}
                      {r.versions.length > 0 && (
                        <select
                          className="version"
                          aria-label={`${r.name} version`}
                          value={version}
                          disabled={props.busy}
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
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        );
      })}
    </>
  );
}
