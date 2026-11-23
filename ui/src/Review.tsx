import { ArrowDownToLine, Check, FileDiff, FileMinus, FilePlus, FileWarning, GitCompareArrows, RefreshCw, Undo2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, type Change, type DiffRow, type Sandbox } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

const KINDS: Record<Change["kind"], string> = { added: "new", deleted: "deleted", modified: "changed" };

/**
 * Review mode: the sandbox works on a copy of the project folder. This shows
 * what it changed there, file by file and line by line, and takes the changes
 * into the project or throws them away.
 */
export function Review(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [changes, setChanges] = useState<Change[] | null>(null);
  const [project, setProject] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(() => {
    if (!sandbox.review) return Promise.resolve();
    setLoading(true);
    return api
      .review(sandbox.name)
      .then((state) => {
        setChanges(state.changes);
        setProject(state.project);
      }, () => {})
      .finally(() => setLoading(false));
  }, [sandbox.name, sandbox.review]);
  useEffect(() => {
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  }, [load]);

  if (!sandbox.review) {
    return (
      <section className="centered">
        <GitCompareArrows size={30} aria-hidden className="faint-icon" />
        <h3>Review what the agent changes</h3>
        <p className="muted readable">
          In review mode the sandbox works on a copy of the project folder. Nothing it does reaches your files until you have looked at it here, line by line,
          and taken it over. What you do not want is thrown away.
        </p>
        {sandbox.workspace ? (
          <div className="actions">
            <button className="primary" onClick={() => act(() => api.setReview(sandbox.name, true))}>
              Switch review mode on
            </button>
          </div>
        ) : (
          <p className="muted">This sandbox has no project folder; review mode is for sandboxes that work on one.</p>
        )}
        {sandbox.running && sandbox.workspace && <p className="muted">The sandbox restarts for it, because what it sees as /workspace changes.</p>}
      </section>
    );
  }

  const list = changes ?? [];
  const mine = list.filter((change) => change.side !== "project");
  const theirs = list.filter((change) => change.side === "project");
  const shown = list.find((change) => change.path === open) ?? list[0];
  const settle = (action: "apply" | "discard" | "bring", paths?: string[]) =>
    act(() =>
      api.settleReview(sandbox.name, action, paths).then((state) => {
        setChanges(state.changes);
      }),
    );

  return (
    <>
      <section className="review-bar">
        <div className="grow">
          <h3>
            {mine.length === 0 ? "Nothing to review" : `${mine.length} file${mine.length === 1 ? "" : "s"} changed by the sandbox`}
            {loading && <RefreshCw size={13} className="spin" aria-hidden />}
          </h3>
          <p className="muted">
            The sandbox works on a copy of <code title={project}>{project.split(/[\\/]/).filter(Boolean).pop()}</code>. {mine.length === 0 ? "Its copy and your project are the same." : "Nothing below is in your project yet."}
          </p>
        </div>
        {mine.some((change) => change.side === "sandbox") && (
          <>
            <button
              onClick={() => confirm("Throw away everything the sandbox changed? Its copy goes back to the state of your project.") && settle("discard")}
            >
              <Undo2 size={15} aria-hidden /> Discard all
            </button>
            <button className="primary" onClick={() => settle("apply")}>
              <Check size={15} aria-hidden /> Take all into the project
            </button>
          </>
        )}
        <button
          className="link"
          title="The sandbox works in the project folder itself again"
          onClick={() => act(() => api.setReview(sandbox.name, false))}
        >
          Switch off
        </button>
      </section>

      {theirs.length > 0 && (
        <section className="notice">
          <ArrowDownToLine size={18} aria-hidden />
          <span className="grow">
            <strong>
              {theirs.length} file{theirs.length === 1 ? " was" : "s were"} changed in your project since the copy was made.
            </strong>
            <small className="muted block">The sandbox still has the old state of {theirs.length === 1 ? "it" : "them"}: {theirs.map((change) => change.path).slice(0, 4).join(", ")}{theirs.length > 4 ? "…" : ""}</small>
          </span>
          <button onClick={() => settle("bring")}>Bring into the sandbox</button>
        </section>
      )}

      {list.length > 0 && shown && (
        <section className="review">
          <ul className="review-files" role="listbox" aria-label="Changed files">
            {list.map((change) => {
              const Icon = change.side === "both" ? FileWarning : change.kind === "added" ? FilePlus : change.kind === "deleted" ? FileMinus : FileDiff;
              const [folder, name] = [change.path.slice(0, change.path.lastIndexOf("/") + 1), change.path.slice(change.path.lastIndexOf("/") + 1)];
              return (
                <li key={change.path}>
                  <button role="option" aria-selected={change.path === shown.path} className={change.path === shown.path ? "active" : ""} onClick={() => setOpen(change.path)}>
                    <Icon size={16} aria-hidden className={`kind-${change.side === "both" ? "both" : change.kind}`} />
                    <span className="grow path">
                      <strong>{name}</strong>
                      {folder && <small>{folder}</small>}
                    </span>
                    {change.side === "project" ? (
                      <span className="chip">in project</span>
                    ) : change.side === "both" ? (
                      <span className="chip paused">both changed</span>
                    ) : (
                      <span className="counts">
                        {change.added > 0 && <b className="plus">+{change.added}</b>}
                        {change.removed > 0 && <b className="minus">−{change.removed}</b>}
                        {change.added + change.removed === 0 && <small>{KINDS[change.kind]}</small>}
                      </span>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
          <FileChange sandbox={sandbox.name} change={shown} settle={settle} />
        </section>
      )}
    </>
  );
}

/** One file: what happened to it, the buttons to decide, and its lines. */
function FileChange(props: { sandbox: string; change: Change; settle: (action: "apply" | "discard" | "bring", paths?: string[]) => Promise<void> }) {
  const { change } = props;
  const [diff, setDiff] = useState<{ rows: DiffRow[]; binary?: boolean; large?: boolean; cut?: boolean } | null>(null);
  useEffect(() => {
    setDiff(null);
    let alive = true;
    api.reviewDiff(props.sandbox, change.path, change.side === "project").then((loaded) => alive && setDiff(loaded), () => {});
    return () => {
      alive = false;
    };
  }, [props.sandbox, change.path, change.added, change.removed, change.kind]);
  const what =
    change.side === "project"
      ? "Changed in your project; the sandbox has the older state"
      : change.side === "both"
        ? "Changed by the sandbox and in your project"
        : change.kind === "added"
          ? "New file from the sandbox"
          : change.kind === "deleted"
            ? "Deleted by the sandbox"
            : "Changed by the sandbox";
  return (
    <div className="review-diff">
      <header>
        <span className="grow">
          <code>{change.path}</code>
          <small className="muted block">{what}</small>
        </span>
        {change.side === "project" ? (
          <button onClick={() => props.settle("bring", [change.path])}>Bring into the sandbox</button>
        ) : (
          <>
            <button onClick={() => props.settle("discard", [change.path])} title="The sandbox's copy of this file goes back to the state of your project">
              <Undo2 size={15} aria-hidden /> Discard
            </button>
            <button className="primary" onClick={() => props.settle("apply", [change.path])} title="This file in your project becomes what the sandbox made of it">
              <Check size={15} aria-hidden /> Take over
            </button>
          </>
        )}
      </header>
      {change.side === "both" && (
        <p className="warning">
          This file changed in your project after the copy was made, and the sandbox changed it too. Taking it over replaces your project's version with the
          sandbox's; discarding replaces the sandbox's with yours. The lines below show your project's version against the sandbox's.
        </p>
      )}
      {!diff ? (
        <p className="muted pad">Comparing…</p>
      ) : diff.binary ? (
        <p className="muted pad">This is not a text file; its content cannot be shown line by line.</p>
      ) : diff.large ? (
        <p className="muted pad">This file is too large to compare line by line here.</p>
      ) : (
        <div className="diff" role="table" aria-label={`Changes in ${change.path}`}>
          {diff.rows.map((row, index) =>
            row.kind === "gap" ? (
              <div key={index} className="gap" role="row">
                ⋯
              </div>
            ) : (
              <div key={index} className={`line is-${row.kind}`} role="row">
                <span className="no" aria-hidden="true">
                  {row.old ?? ""}
                </span>
                <span className="no" aria-hidden="true">
                  {row.new ?? ""}
                </span>
                <span className="mark" aria-hidden="true">
                  {row.kind === "add" ? "+" : row.kind === "del" ? "−" : ""}
                </span>
                <code>{row.text || " "}</code>
              </div>
            ),
          )}
          {diff.cut && <p className="muted pad">The rest of this file's changes is not shown.</p>}
        </div>
      )}
    </div>
  );
}
