import { FitAddon } from "@xterm/addon-fit";
import { Terminal as Xterm } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { token } from "./api";

export interface Session {
  id: number;
  sandbox: string;
  title: string;
  /** Command to run; a login shell when absent. */
  cmd?: string;
}

/** One terminal in a sandbox. It stays connected while hidden. */
export function Terminal(props: { session: Session; visible: boolean; onExit: () => void }) {
  const host = useRef<HTMLDivElement>(null);
  const fit = useRef<FitAddon | null>(null);
  const { session, visible, onExit } = props;

  useEffect(() => {
    const term = new Xterm({ fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace', fontSize: 13, cursorBlink: true, scrollback: 5000 });
    const fitter = new FitAddon();
    fit.current = fitter;
    term.loadAddon(fitter);
    term.open(host.current!);
    fitter.fit();

    const query = new URLSearchParams({ token, cols: String(term.cols), rows: String(term.rows) });
    if (session.cmd) query.set("cmd", session.cmd);
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const socket = new WebSocket(`${scheme}://${location.host}/api/sandboxes/${encodeURIComponent(session.sandbox)}/terminal?${query}`);
    socket.binaryType = "arraybuffer";
    const encoder = new TextEncoder();
    let opened = false;
    socket.onopen = () => {
      opened = true;
      term.focus();
    };
    socket.onmessage = (message) => term.write(new Uint8Array(message.data as ArrayBuffer));
    socket.onclose = () => {
      term.write(opened ? "\r\n\x1b[2m[session ended]\x1b[0m\r\n" : "\r\n\x1b[31mCould not open a terminal. Is the sandbox able to start?\x1b[0m\r\n");
      if (opened) onExit();
    };
    const input = term.onData((data) => socket.readyState === WebSocket.OPEN && socket.send(encoder.encode(data)));
    const resize = term.onResize(({ cols, rows }) => socket.readyState === WebSocket.OPEN && socket.send(JSON.stringify({ resize: [cols, rows] })));
    const observer = new ResizeObserver(() => host.current?.offsetParent && fitter.fit());
    observer.observe(host.current!);

    return () => {
      observer.disconnect();
      input.dispose();
      resize.dispose();
      socket.onclose = null;
      socket.close();
      term.dispose();
    };
    // A session never changes after it is created.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.id]);

  useEffect(() => {
    if (visible) fit.current?.fit();
  }, [visible]);

  return <div ref={host} className="terminal" style={{ display: visible ? "block" : "none" }} />;
}
