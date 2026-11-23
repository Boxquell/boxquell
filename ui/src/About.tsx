import { ExternalLink, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type AboutInfo, type UpdateInfo } from "./api";

/** Shows text the way it is written, with headings and lists of a changelog made readable. */
function Changelog(props: { text: string }) {
  return (
    <div className="prose">
      {props.text.split("\n").map((line, index) => {
        if (line.startsWith("### ")) return <h4 key={index}>{line.slice(4)}</h4>;
        if (line.startsWith("## ")) return <h3 key={index}>{line.slice(3)}</h3>;
        if (line.startsWith("# ")) return null;
        if (line.startsWith("- ")) return <li key={index}>{line.slice(2)}</li>;
        return line.trim() ? <p key={index}>{line.replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")}</p> : null;
      })}
    </div>
  );
}

export function About() {
  const [about, setAbout] = useState<AboutInfo | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [notices, setNotices] = useState<string | null>(null);

  useEffect(() => {
    api.about().then(setAbout, () => {});
  }, []);
  const check = () => {
    setChecking(true);
    setProblem(null);
    api
      .updateCheck()
      .then(setUpdate, (err) => setProblem(String(err.message ?? err)))
      .finally(() => setChecking(false));
  };

  return (
    <>
      <section>
        <div className="about-head">
          <img src="/icon.svg" alt="" width="56" height="56" />
          <div className="grow">
            <h3 className="title">Boxquell {about?.version}</h3>
            <p className="muted">MicroVM sandboxes for coding agents. Licensed under {about?.license ?? "MIT OR Apache-2.0"}.</p>
          </div>
          <button onClick={check} disabled={checking}>
            <RefreshCw size={15} aria-hidden="true" /> {checking ? "Checking…" : "Check for updates"}
          </button>
        </div>
        {update && (
          <p className={update.newer ? "warning" : "muted"}>
            {update.newer ? (
              <>
                Version {update.latest} is available.{" "}
                <a href={update.url} target="_blank" rel="noreferrer">
                  Open the release <ExternalLink size={13} aria-hidden="true" />
                </a>
              </>
            ) : (
              (update.note ?? `You have the latest version (${update.current}).`)
            )}
          </p>
        )}
        {problem && <p className="bad">The check did not work: {problem}</p>}
        <p className="muted">
          Checking asks GitHub for the latest release, and only when you press the button.{" "}
          {about && (
            <a href={about.repository} target="_blank" rel="noreferrer">
              Project page <ExternalLink size={13} aria-hidden="true" />
            </a>
          )}
        </p>
      </section>

      <section>
        <h3>What is new</h3>
        {about ? <Changelog text={about.changelog} /> : <p className="muted">Loading…</p>}
      </section>

      <section>
        <h3>Third-party notices</h3>
        <p className="muted">Boxquell is built from open-source components. Their licenses are listed here.</p>
        {notices === null ? (
          <div className="actions left">
            <button onClick={() => api.notices().then((answer) => setNotices(answer.text), () => setNotices("The notices could not be loaded."))}>Show the notices</button>
          </div>
        ) : (
          <pre className="viewer">{notices}</pre>
        )}
      </section>
    </>
  );
}
