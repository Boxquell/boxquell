import { RefreshCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";

/** The end of a log file, reloaded on request or every few seconds. */
export function LogViewer(props: { title: string; about: string; load: () => Promise<{ text: string }> }) {
  const [text, setText] = useState<string | null>(null);
  const [follow, setFollow] = useState(true);
  const view = useRef<HTMLPreElement>(null);
  const load = () => props.load().then((answer) => setText(answer.text), () => setText("The log could not be loaded."));

  useEffect(() => {
    load();
    if (!follow) return;
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [follow]);
  useEffect(() => {
    if (follow && view.current) view.current.scrollTop = view.current.scrollHeight;
  }, [text, follow]);

  return (
    <section>
      <h3>
        <span className="grow">{props.title}</span>
        <label className="check inline">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> follow
        </label>
        <button onClick={load}>
          <RefreshCw size={15} aria-hidden="true" /> Reload
        </button>
      </h3>
      <p className="muted">{props.about}</p>
      <pre className="viewer" ref={view}>
        {text === null ? "Loading…" : text || "Nothing has been written yet."}
      </pre>
    </section>
  );
}
