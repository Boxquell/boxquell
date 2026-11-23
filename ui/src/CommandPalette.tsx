import { Search, type LucideIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

/** Something to jump to or to do, offered when typing in the search. */
export interface Command {
  id: string;
  title: string;
  /** Where it belongs, e.g. a sandbox's name; shown at the right. */
  hint?: string;
  /** Other words that find it. */
  keywords?: string;
  icon: LucideIcon;
  run: () => void;
}

/**
 * Find anything: a sandbox, a page, an action. Opens with Ctrl+K or ⌘K, takes
 * a few letters, and runs the chosen entry with Enter.
 */
export function CommandPalette(props: { open: boolean; onClose: () => void; commands: Command[] }) {
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const found = useMemo(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return props.commands.filter((command) => words.every((word) => `${command.title} ${command.hint ?? ""} ${command.keywords ?? ""}`.toLowerCase().includes(word))).slice(0, 40);
  }, [query, props.commands]);
  useEffect(() => {
    if (props.open) {
      setQuery("");
      setAt(0);
    }
  }, [props.open]);
  useEffect(() => {
    list.current?.querySelector(".on")?.scrollIntoView({ block: "nearest" });
  }, [at]);
  if (!props.open) return null;
  const run = (command: Command | undefined) => {
    if (!command) return;
    props.onClose();
    command.run();
  };
  return (
    <div className="palette-backdrop" role="presentation" onClick={props.onClose}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Find a sandbox, a page or an action" onClick={(event) => event.stopPropagation()}>
        <div className="palette-input">
          <Search size={18} aria-hidden />
          <input
            autoFocus
            placeholder="Find a sandbox, a page or an action…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setAt(0);
            }}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown") setAt((now) => Math.min(found.length - 1, now + 1));
              else if (event.key === "ArrowUp") setAt((now) => Math.max(0, now - 1));
              else if (event.key === "Enter") run(found[at]);
              else if (event.key === "Escape") props.onClose();
              else return;
              event.preventDefault();
            }}
            aria-label="Search"
          />
        </div>
        {found.length === 0 ? (
          <p className="none">Nothing matches “{query}”.</p>
        ) : (
          <ul ref={list} role="listbox">
            {found.map((command, index) => (
              <li key={command.id} role="option" aria-selected={index === at}>
                <button className={index === at ? "on" : ""} onMouseMove={() => setAt(index)} onClick={() => run(command)}>
                  <command.icon size={17} aria-hidden />
                  {command.title}
                  {command.hint && <small>{command.hint}</small>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
