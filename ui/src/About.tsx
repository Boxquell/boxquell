import { Bug, ExternalLink, Lightbulb, RefreshCw, ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type AboutInfo, type UpdateInfo } from "./api";

interface Release {
  version: string;
  date: string | null;
  intro: string[];
  groups: { title: string; items: string[] }[];
}

/** Read a changelog in the "Keep a Changelog" form into releases, groups and entries. */
function parseChangelog(text: string): Release[] {
  const releases: Release[] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    const release = releases[releases.length - 1];
    if (line.startsWith("## ")) {
      const [version, date] = line.slice(3).split(" - ");
      releases.push({ version: version.replace(/[[\]]/g, "").trim(), date: date?.trim() ?? null, intro: [], groups: [] });
    } else if (!release || !line) {
      continue;
    } else if (line.startsWith("### ")) {
      release.groups.push({ title: line.slice(4), items: [] });
    } else if (line.startsWith("- ")) {
      if (release.groups.length === 0) release.groups.push({ title: "", items: [] });
      release.groups[release.groups.length - 1].items.push(line.slice(2));
    } else {
      release.intro.push(line);
    }
  }
  // An empty "Unreleased" heading says nothing.
  return releases.filter((release) => release.intro.length > 0 || release.groups.some((group) => group.items.length > 0));
}

/** Inline code in changelog text. */
function Inline(props: { text: string }) {
  const parts = props.text.replace(/\[([^\]]+)\]\([^)]+\)/g, "$1").split("`");
  return <>{parts.map((part, index) => (index % 2 === 1 ? <code key={index}>{part}</code> : part))}</>;
}

function Changelog(props: { text: string; current?: string }) {
  const releases = parseChangelog(props.text);
  if (releases.length === 0) return <p className="muted">No changes are recorded yet.</p>;
  return (
    <div className="changelog">
      {releases.map((release, index) => (
        <details key={release.version} open={index === 0} className="release">
          <summary>
            <strong>{release.version === "Unreleased" ? "Coming next" : `Version ${release.version}`}</strong>
            {release.version === props.current && <span className="chip on">installed</span>}
            {release.date && <span className="muted">{new Date(release.date).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" })}</span>}
          </summary>
          {release.intro.map((line) => (
            <p key={line}>
              <Inline text={line} />
            </p>
          ))}
          <div className="release-groups">
            {release.groups.map((group) => (
              <div key={group.title}>
                {group.title && <h4>{group.title}</h4>}
                <ul>
                  {group.items.map((item) => (
                    <li key={item}>
                      <Inline text={item} />
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </details>
      ))}
    </div>
  );
}

const REPOSITORY = "https://github.com/Boxquell/boxquell";

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
        <h3>Help and feedback</h3>
        <p className="muted">Both open a form on GitHub in your browser. Nothing is sent before you submit it there.</p>
        <div className="actions left">
          <a className="button" href={`${about?.repository ?? REPOSITORY}/issues/new?template=bug.yml&system=${encodeURIComponent(`boxquell ${about?.version ?? ""}`)}`} target="_blank" rel="noreferrer">
            <Bug size={15} aria-hidden="true" /> Report a bug
          </a>
          <a className="button" href={`${about?.repository ?? REPOSITORY}/issues/new?template=feature.yml`} target="_blank" rel="noreferrer">
            <Lightbulb size={15} aria-hidden="true" /> Request a feature
          </a>
          <a className="button" href={`${about?.repository ?? REPOSITORY}/security/advisories/new`} target="_blank" rel="noreferrer" title="For ways around a sandbox boundary: reported privately">
            <ShieldAlert size={15} aria-hidden="true" /> Report a security problem
          </a>
        </div>
      </section>

      <section>
        <h3>What is new</h3>
        {about ? <Changelog text={about.changelog} current={about.version} /> : <p className="muted">Loading…</p>}
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
