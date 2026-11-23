import { ChevronUp, Folder, FolderOpen, Home } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type FolderListing } from "./api";

/**
 * A text field for a folder of this computer with a browser beside it.
 * The path can still be typed or pasted; the browser only saves the typing.
 */
export function FolderField(props: { label: string; value: string; onChange: (path: string) => void; placeholder?: string; required?: boolean; hidden?: boolean }) {
  const [open, setOpen] = useState(false);
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [showHidden, setShowHidden] = useState(props.hidden ?? false);
  const [error, setError] = useState<string | null>(null);

  const go = (path?: string) =>
    api.folders(path, showHidden).then(
      (next) => {
        setListing(next);
        setError(null);
      },
      (err) => setError(String(err.message ?? err)),
    );
  useEffect(() => {
    if (open) go(listing?.path ?? (props.value.trim() || undefined));
    // Reload when the browser opens or hidden folders are switched.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, showHidden]);

  return (
    <div className="folder-field">
      <label>
        {props.label}
        <span className="row">
          <input className="grow" required={props.required} placeholder={props.placeholder} value={props.value} onChange={(e) => props.onChange(e.target.value)} />
          <button type="button" onClick={() => setOpen(!open)} aria-expanded={open}>
            <FolderOpen size={15} aria-hidden="true" /> {open ? "Close" : "Choose…"}
          </button>
        </span>
      </label>
      {open && (
        <div className="folder-browser">
          <div className="folder-bar">
            <button type="button" title="Home folder" aria-label="Home folder" onClick={() => go(listing?.home ?? undefined)}>
              <Home size={15} />
            </button>
            <button type="button" title="One folder up" aria-label="One folder up" disabled={!listing?.parent} onClick={() => listing?.parent && go(listing.parent)}>
              <ChevronUp size={15} />
            </button>
            <code className="grow">{listing?.path ?? "…"}</code>
            <label className="check inline">
              <input type="checkbox" checked={showHidden} onChange={(e) => setShowHidden(e.target.checked)} /> hidden
            </label>
            <button
              type="button"
              className="primary"
              disabled={!listing}
              onClick={() => {
                if (listing) props.onChange(listing.path);
                setOpen(false);
              }}
            >
              Use this folder
            </button>
          </div>
          {error && <p className="bad">{error}</p>}
          <ul>
            {listing?.folders.map((name) => (
              <li key={name}>
                <button type="button" onClick={() => go(`${listing.path === "/" ? "" : listing.path}/${name}`)}>
                  <Folder size={15} aria-hidden="true" /> {name}
                </button>
              </li>
            ))}
            {listing && listing.folders.length === 0 && <li className="muted pad">No folders in here.</li>}
          </ul>
        </div>
      )}
    </div>
  );
}
