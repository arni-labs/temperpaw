import { start, useApp, useRecords, useRecord, move, can, who, ago, howItWorks } from "@katagami/app";
import { Routes, Route, Link, useParams } from "react-router";
import { useState } from "react";
import "./app.css";

type Entry = { role: string; content: string; call_id: string; call_ids: string[]; call_names: string[]; call_args: string[] };

function list<T>(v: unknown): T[] {
  if (Array.isArray(v)) return v as T[];
  if (typeof v === "string" && v.trim().startsWith("[")) {
    try {
      return JSON.parse(v) as T[];
    } catch {
      return [];
    }
  }
  return [];
}

function commandOf(args: string): string {
  try {
    const a = JSON.parse(args);
    if (a && typeof a.command === "string") return a.command;
  } catch {}
  return args;
}

function placeLabel(place: string): string {
  return katagami.label("session", "place", place) || place;
}

function Sessions() {
  const { app } = useApp();
  const sessions = useRecords("session", { limit: 30 });
  const agents = useRecords("agent", { limit: 12 });
  return (
    <main className="paw">
      <header className="paw-head">
        <h1>{app.name}</h1>
        <button type="button" className="paw-quiet" onClick={howItWorks}>How it works</button>
        <k-key thing="agent" move="create">New agent</k-key>
      </header>
      <section className="paw-agents">
        {agents.map((a) => (
          <div key={a.id} data-record={a.id} className="paw-agent">
            <span className="paw-agent-name">{a.title || a.fields.name}</span>
            <code className="paw-model">{a.fields.provider}/{a.fields.model}</code>
            <k-key thing="agent" record={a.id} move="start_session">New session</k-key>
          </div>
        ))}
      </section>
      <ol className="paw-sessions" data-part="rows">
        {sessions.map((s) => {
          const h = list<Entry>(s.fields.history);
          const last = s.fields.reply || (h.length ? h[h.length - 1].content : "");
          return (
            <li key={s.id} data-record={s.id} data-place={s.place}>
              <Link to={`/session/${s.id}`} className="paw-session-link">
                <span className="paw-session-title">{s.fields.title || "Untitled session"}</span>
                <mark data-tone={s.place === "Failed" ? "notnow" : s.fresh ? "new" : "here"}>{placeLabel(s.place)}</mark>
                <span className="paw-session-last">{String(last).slice(0, 140)}</span>
                <span className="paw-by">{ago(s.moved_at)}</span>
              </Link>
            </li>
          );
        })}
      </ol>
    </main>
  );
}

function Tape({ history }: { history: Entry[] }) {
  return (
    <ol className="paw-tape">
      {history.map((e, i) => (
        <li key={i} className="paw-line" data-role={e.role}>
          {e.role === "tool" ? (
            <pre className="paw-out">{e.content}</pre>
          ) : (
            <p className="paw-said">{e.content}</p>
          )}
          {e.role === "assistant" && e.call_ids && e.call_ids.length > 0 && (
            <ul className="paw-calls">
              {e.call_ids.map((c, k) => (
                <li key={c}>
                  <span className="paw-tool">{e.call_names[k]}</span>
                  <code>{commandOf(e.call_args[k] || "")}</code>
                </li>
              ))}
            </ul>
          )}
        </li>
      ))}
    </ol>
  );
}

function Live({ s }: { s: any }) {
  const q = list<string>(s.fields.queue_ids);
  const names = list<string>(s.fields.queue_names);
  const args = list<string>(s.fields.queue_args);
  const head = q[0];
  if (s.place === "WaitingApproval" && head) {
    return (
      <section className="paw-gate">
        <p>The agent wants to run <span className="paw-tool">{names[0]}</span>:</p>
        <pre className="paw-out">{commandOf(args[0] || "")}</pre>
        <div className="paw-keys">
          <button type="button" className="paw-key" onClick={() => move("session", s.id, "approve", { call: head })}>Approve</button>
          <button type="button" className="paw-key paw-quiet" onClick={() => move("session", s.id, "deny", { call: head })}>Deny</button>
        </div>
      </section>
    );
  }
  if (s.place === "Thinking" || s.place === "Compacting") {
    return <p className="paw-pulse">{s.place === "Compacting" ? "Summarising the conversation." : `${s.fields.model} is thinking.`}</p>;
  }
  if ((s.place === "Acting" || s.place === "Calling") && head) {
    return (
      <p className="paw-pulse">
        Running on the computer: <code>{commandOf(args[0] || "")}</code>
      </p>
    );
  }
  if (s.fields.why) return <p className="paw-why"><mark data-tone="notnow">Not now</mark> {s.fields.why}</p>;
  return null;
}

function Computer({ s }: { s: any }) {
  const has = !!s.fields.computer;
  const paused = s.fields.computer_paused === true || s.fields.computer_paused === "true";
  return (
    <aside className="paw-computer" data-state={has ? (paused ? "paused" : "ready") : "none"}>
      <span className="paw-led" aria-hidden="true"></span>
      <span>{has ? (paused ? "Computer paused, workspace kept" : "Computer running") : "No computer yet: the first command starts one"}</span>
      <span className="paw-by">{Number(s.fields.tokens_in || 0) + Number(s.fields.tokens_out || 0)} tokens</span>
    </aside>
  );
}

function Ask({ s }: { s: any }) {
  const [text, setText] = useState("");
  const [said, setSaid] = useState("");
  const send = async () => {
    if (!text.trim()) return;
    const r = await move("session", s.id, "ask", { text });
    if (r.ok) {
      setText("");
      setSaid("");
    } else setSaid(r.reason);
  };
  return (
    <div className="paw-ask" data-part="well">
      <textarea
        data-part="input"
        rows={2}
        value={text}
        placeholder={can(s, "ask") ? "Ask the agent" : "The agent is working"}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) send();
        }}
      />
      <button type="button" className="paw-key" onClick={send}>Ask</button>
      {said && <p className="paw-why"><mark data-tone="notnow">Not now</mark> {said}</p>}
    </div>
  );
}

function Session() {
  const { id = "" } = useParams();
  const s = useRecord("session", id);
  if (!s) return <main className="paw"><Link to="/">All sessions</Link></main>;
  const history = list<Entry>(s.fields.history);
  return (
    <main className="paw paw-console" data-record={s.id} data-place={s.place}>
      <header className="paw-head">
        <Link to="/">All sessions</Link>
        <h1>{s.fields.title || "Untitled session"}</h1>
        <mark data-tone={s.place === "Failed" ? "notnow" : "here"}>{placeLabel(s.place)}</mark>
        <code className="paw-model">{s.fields.provider}/{s.fields.model}</code>
      </header>
      <Computer s={s} />
      <Tape history={history} />
      <Live s={s} />
      <Ask s={s} />
      <footer className="paw-foot">
        <span className="paw-by">Moved by {who(s.moved_by)}, {ago(s.moved_at)}.</span>
        <k-moves thing="session" record={s.id} only="swap_model close"></k-moves>
      </footer>
    </main>
  );
}

start(
  <Routes>
    <Route path="/" element={<Sessions />} />
    <Route path="/session/:id" element={<Session />} />
  </Routes>,
);
