import { CircleCheck, CircleStop, CircleX, LoaderCircle, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, type Recipe, type Sandbox, type Task } from "./api";
import { recipeName } from "./RecipeCards";

type Act = (action: () => Promise<unknown>) => Promise<void>;

function duration(task: Task): string {
  const seconds = Math.max(0, Math.round(((task.ended_ms ?? Date.now()) - task.started_ms) / 1000));
  if (seconds < 60) return `${seconds} s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
  return `${Math.floor(seconds / 3600)} h ${Math.floor((seconds % 3600) / 60)} min`;
}

const STATES: Record<string, string> = { running: "working", done: "finished", failed: "ended with an error", stopped: "stopped", interrupted: "interrupted" };

/** Tasks an agent works through in the background, without a terminal: give one, watch its log, stop it. */
export function Tasks(props: { sandbox: Sandbox; recipes: Recipe[]; act: Act; openSoftware: () => void; openSettings: () => void }) {
  const { sandbox, recipes, act } = props;
  const agents = sandbox.recipes.map((id) => recipes.find((r) => r.id === recipeName(id))).filter((r): r is Recipe => !!r?.task);
  const [agent, setAgent] = useState("");
  const [prompt, setPrompt] = useState("");
  const [snapshot, setSnapshot] = useState(true);
  const tasks = sandbox.tasks ?? [];
  const [open, setOpen] = useState<string | null>(null);
  const chosen = agents.find((candidate) => candidate.id === agent) ?? agents[0];

  return (
    <>
      <section>
        <h3>Give a task</h3>
        {agents.length === 0 ? (
          <>
            <p className="muted">
              An agent that can work without a terminal takes a task here and works through it in the background, in the project folder. None of the agents in
              this sandbox can do that yet.
            </p>
            <div className="actions left">
              <button onClick={props.openSoftware}>Open Software</button>
            </div>
          </>
        ) : (
          <form
            className="task-form"
            onSubmit={(event) => {
              event.preventDefault();
              act(() => api.startTask(sandbox.name, chosen.id, prompt, snapshot)).then(() => setPrompt(""));
            }}
          >
            <textarea
              required
              rows={4}
              placeholder="What should the agent do? For example: add tests for the parser and make them pass."
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              aria-label="The task"
            />
            <div className="row">
              {agents.length > 1 && (
                <select value={chosen.id} onChange={(e) => setAgent(e.target.value)} aria-label="Agent">
                  {agents.map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.name}
                    </option>
                  ))}
                </select>
              )}
              <label className="check grow">
                <input type="checkbox" checked={snapshot} onChange={() => setSnapshot(!snapshot)} />
                Take a snapshot of the sandbox first
              </label>
              <button className="primary">
                <Play size={15} aria-hidden /> Start {agents.length === 1 ? `with ${chosen.name}` : ""}
              </button>
            </div>
            <p className="muted">
              The agent works on its own and is not asked before it changes files or runs commands; the sandbox is what holds it. It works in the project
              folder, so its changes land in your files. A snapshot covers the sandbox, not the project folder: keep that under version control.{" "}
              {snapshot && sandbox.running ? "For the snapshot the sandbox stops for a moment. " : ""}
              You get a notification when the task is over.
            </p>
          </form>
        )}
      </section>
      {tasks.length > 0 && (
        <section>
          <h3>Tasks</h3>
          <ul className="tasks">
            {tasks.map((task) => (
              <li key={task.id} className={task.state}>
                <button className="task-head" onClick={() => setOpen(open === task.id ? null : task.id)} aria-expanded={open === task.id}>
                  {task.state === "running" ? (
                    <LoaderCircle size={18} className="spin" aria-hidden />
                  ) : task.state === "done" ? (
                    <CircleCheck size={18} className="good" aria-hidden />
                  ) : task.state === "stopped" ? (
                    <CircleStop size={18} aria-hidden />
                  ) : (
                    <CircleX size={18} className="bad" aria-hidden />
                  )}
                  <span className="grow">
                    <strong>{task.prompt}</strong>
                    <small className="muted">
                      {task.agent_name} · {STATES[task.state] ?? task.state} · {new Date(task.started_ms).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })} · {duration(task)}
                    </small>
                  </span>
                </button>
                <span className="task-actions">
                  {task.state === "running" ? (
                    <button onClick={() => act(() => api.stopTask(sandbox.name, task.id))}>Stop</button>
                  ) : (
                    <>
                      {task.snapshot && (
                        <button className="link" onClick={props.openSettings} title="The snapshot taken before this task is under Settings">
                          snapshot
                        </button>
                      )}
                      <button className="link" onClick={() => act(() => api.deleteTask(sandbox.name, task.id))}>
                        remove
                      </button>
                    </>
                  )}
                </span>
                {open === task.id && <TaskLog sandbox={sandbox.name} task={task} />}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

/** What a task printed, followed while it runs. */
function TaskLog(props: { sandbox: string; task: Task }) {
  const [text, setText] = useState("");
  const view = useRef<HTMLPreElement>(null);
  const running = props.task.state === "running";
  useEffect(() => {
    let alive = true;
    const load = () => api.taskLog(props.sandbox, props.task.id).then((log) => alive && setText(log.text), () => {});
    load();
    const timer = running ? setInterval(load, 1500) : undefined;
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [props.sandbox, props.task.id, running]);
  useEffect(() => {
    if (view.current) view.current.scrollTop = view.current.scrollHeight;
  }, [text]);
  return (
    <pre className="task-log" ref={view} aria-label="What the task printed">
      {text || (running ? "Nothing printed yet…" : "It printed nothing.")}
    </pre>
  );
}
