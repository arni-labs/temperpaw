// TemperPaw on Temper V2, rebuilt from scratch (26 Nov 2026): an agent harness as six small record types.
//   Agent     who the agent is: its soul, its model and provider (swappable) and its tools.
//   Session   one durable conversation: a state machine that survives restarts, model swaps and a lost computer.
//   Turn      one immutable entry of the conversation (user, assistant, tool, summary), parent-linked.
//   ToolCall  one governed tool call the model asked for, decided from the agent's grants: bash runs on the Computer,
//             unikon calls UniKon's connector through unikon_door, with no computer.
//   Computer  the session's own Tensorlake sandbox (never the node's), reached only through computer_door.
//   Memory    what an agent keeps: (key, scope, content), Active or Archived.
// The loop is the Session's statuses. V2 picks a move's follow-on only from the status that move leaves, so "running"
// is four statuses: Thinking (the model is asked), Calling (a tool call is next), Acting (a bash call runs on the
// computer) and Connecting (a unikon call runs at UniKon's connector).
//
//   Idle|Failed --ask--> Thinking --answered_tools--> Calling --next_tool--> Acting | Connecting | WaitingApproval
//   Thinking --answered--> Idle | Compacting --compacted--> Idle
//   Acting --tool_done | tool_refused | tool_failed--> Calling (more calls) | Thinking (next turn) | Idle (step limit)
//   Connecting --tool_answered | tool_failed--> the same
//   WaitingApproval --approve--> Acting | Connecting ; --deny--> Calling | Thinking | Idle
//   Idle --10 min (status timer)--> pause_computer --computer_paused-->        Idle|Failed --close--> Done
//
// Trajectory capture is the kernel's own: every move is a hash-chained commit in the app's history.

module turn_record {
  import temper.* from "./temper"

  type TurnId = Id
  type Status = | Written
  // role: user, assistant, tool or summary. parent: the turn before it in this session ("turn-0" for the first).
  // tool: for a tool turn, the call it answers; for an assistant turn, the calls it asked for, comma-joined by the model.
  type Row = { status: Status, parent: str, role: str, content: str, tool: str, model: str, tokens_in: int, tokens_out: int }
  type Msg = NoMsgs
  type Ask = NoAsks

  pure val record: RecordDecl = { name: "Turn", status: "status" }
  pure val moves: List[MoveDecl[Status]] = []
  pure val marks: List[Mark] = [ChildOf("Session")]
  // temper:generated begin
  // temper:generated end
}

module toolcall_record {
  import temper.* from "./temper"

  type ToolCallId = Id
  type Status = Waiting | Running | Done | Failed | Denied
  type Row = { status: Status, turn: str, tool: str, args: str, exit_code: int, output: str }
  type Msg = NoMsgs
  type Ask = NoAsks

  pure val record: RecordDecl = { name: "ToolCall", status: "status" }
  pure val moves: List[MoveDecl[Status]] = []
  pure val marks: List[Mark] = [ChildOf("Session")]
  // temper:generated begin
  // temper:generated end
}

module computer_record {
  import temper.* from "./temper"

  type ComputerId = Id
  // Ready: running. Paused: suspended by Tensorlake, its disk kept; the next call resumes it (about 0.5 s). Ended: deleted.
  type Status = Ready | Paused | Ended
  type Row = { status: Status, sandbox: str }
  type Msg = NoMsgs
  type Ask = NoAsks

  pure val record: RecordDecl = { name: "Computer", status: "status" }
  pure val moves: List[MoveDecl[Status]] = []
  pure val marks: List[Mark] = [ChildOf("Session")]
  // temper:generated begin
  // temper:generated end
}

module memory_record {
  import temper.* from "./temper"

  type MemoryId = Id
  type Status = Active | Archived
  type Row = { status: Status, key: str, scope: str, content: str, pinned: bool }
  type Msg = NoMsgs
  type Ask = NoAsks
  type RememberIn = { key: str, scope: str, content: str }
  type ReviseIn = { content: str }

  pure val record: RecordDecl = { name: "Memory", status: "status" }
  pure val moves: List[MoveDecl[Status]] = [
    { name: "remember", from: FirstMove },
    { name: "revise", from: Statuses([Active]) },
    { name: "archive", from: Statuses([Active]) },
    { name: "restore", from: Statuses([Archived]) },
  ]
  pure def remember_guard(i: RememberIn, c: Ctx): bool = require(i.key != "" and i.content != "", "A memory has a key and content.")
  pure def remember_apply(i: RememberIn, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: { status: Active, key: i.key, scope: i.scope, content: i.content, pinned: false }, follow: [],
  }
  pure def revise_guard(t: Row, i: ReviseIn, c: Ctx): bool = require(i.content != "", "A memory keeps some content.")
  pure def revise_apply(t: Row, i: ReviseIn, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, content: i.content }, follow: [] }
  pure def archive_guard(t: Row, i: NoInput, c: Ctx): bool = true
  pure def archive_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, status: Archived }, follow: [] }
  pure def restore_guard(t: Row, i: NoInput, c: Ctx): bool = true
  pure def restore_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, status: Active }, follow: [] }
  // temper:generated begin
  // temper:generated end
}

module catalog_record {
  import temper.* from "./temper"

  // The models a person can pick from: OpenRouter's live list of models that take tool calls, read by model_catalog.
  // Fetching: the list is being read (a refresh keeps the last list readable). Ready: models holds it. Failed: the
  // read failed; refresh tries again.
  type CatalogId = Id
  type Status = Fetching | Ready | Failed
  // models: one line per model, id TAB name TAB context length TAB prompt price TAB completion price (per token, as
  // OpenRouter writes them).
  type Row = { status: Status, models: str, count: int }
  type Msg = NoMsgs
  type CatalogIn = { source: str }
  type CatalogOut = { models: str, count: int }
  type Ask = | AskCatalog(CatalogIn)

  pure val record: RecordDecl = { name: "Catalog", status: "status" }
  pure val moves: List[MoveDecl[Status]] = [
    { name: "fetch", from: FirstMove },
    { name: "refresh", from: Statuses([Ready, Failed]) },
    { name: "filled", from: Statuses([Fetching]) },
    { name: "fetch_failed", from: Statuses([Fetching]) },
  ]
  pure val catalog_paths: ModulePaths = { applied: ["filled"], failed: ["fetch_failed"], unknown: ["fetch_failed"], refused: ["fetch_failed"] }
  pure val read_list: Ask = AskCatalog({ source: "openrouter" })
  pure def fetch_guard(i: NoInput, c: Ctx): bool = true
  pure def fetch_apply(i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: { status: Fetching, models: "", count: 0 },
    follow: [RunModule({ mod: "model_catalog", input: read_list, paths: catalog_paths })],
  }
  pure def refresh_guard(t: Row, i: NoInput, c: Ctx): bool = true
  pure def refresh_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: { ...t, status: Fetching },
    follow: [RunModule({ mod: "model_catalog", input: read_list, paths: catalog_paths })],
  }
  pure def filled_guard(t: Row, i: CatalogOut, c: Ctx): bool = require(by_module_answer(c), "Only model_catalog's answer fills the list.")
  pure def filled_apply(t: Row, i: CatalogOut, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: { status: Ready, models: i.models, count: i.count }, follow: [],
  }
  pure def fetch_failed_guard(t: Row, i: NoInput, c: Ctx): bool = require(by_module_answer(c), "Only model_catalog's end fails a read.")
  pure def fetch_failed_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, status: Failed }, follow: [] }
  // temper:generated begin
  // temper:generated end
}

module agent_record {
  import temper.* from "./temper"

  type AgentId = Id
  type Status = Ready | Paused
  // provider: OpenAI, Anthropic or OpenRouter (any OpenAI-compatible model), each its own destination and key; the row
  // keeps its wire name, which turn_runner reads. tools: the tools it may call ("bash" runs on its session's computer).
  // approve: the ones a person approves each time.
  type Provider = OpenAI | Anthropic | OpenRouter
  type Row = { status: Status, name: str, soul: str, provider: str, model: str, tools: Set[str], approve: Set[str], sessions: int }
  type Msg = DoOpen({ title: str, soul: str, provider: str, model: str, tools: Set[str], approve: Set[str] })
  type Ask = NoAsks
  type SetIn = { name: str, soul: str, provider: Provider, model: str, tools: Set[str], approve: Set[str] }
  type StartIn = { title: str }

  pure val record: RecordDecl = { name: "Agent", status: "status" }
  pure val moves: List[MoveDecl[Status]] = [
    { name: "create", from: FirstMove },
    { name: "revise", from: Statuses([Ready, Paused]) },
    { name: "start_session", from: Statuses([Ready]) },
    { name: "pause", from: Statuses([Ready]) },
    { name: "resume", from: Statuses([Paused]) },
  ]
  pure def provider_name(p: Provider): str = if (p == OpenAI) "openai" else if (p == Anthropic) "anthropic" else "openrouter"
  pure def create_guard(i: SetIn, c: Ctx): bool = and {
    require(i.name != "" and i.model != "", "An agent has a name and a model."),
  }
  pure def create_apply(i: SetIn, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: {
      status: Ready, name: i.name, soul: i.soul, provider: provider_name(i.provider), model: i.model, tools: i.tools,
      approve: i.approve, sessions: 0,
    },
    follow: [],
  }
  // Swapping the model or provider here changes the sessions it starts next; a live session swaps with swap_model.
  pure def revise_guard(t: Row, i: SetIn, c: Ctx): bool = and {
    require(i.name != "" and i.model != "", "An agent has a name and a model."),
  }
  pure def revise_apply(t: Row, i: SetIn, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: {
      ...t, name: i.name, soul: i.soul, provider: provider_name(i.provider), model: i.model, tools: i.tools, approve: i.approve,
    },
    follow: [],
  }
  pure def start_session_guard(t: Row, i: StartIn, c: Ctx): bool = true
  pure def start_session_apply(t: Row, i: StartIn, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: { ...t, sessions: t.sessions + 1 },
    follow: [Make({
      record: "Session", id: minted_id(c),
      msg: DoOpen({
        title: i.title, soul: t.soul, provider: t.provider, model: t.model, tools: t.tools, approve: t.approve,
      }),
      paths: no_paths,
    })],
  }
  pure def pause_guard(t: Row, i: NoInput, c: Ctx): bool = true
  pure def pause_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, status: Paused }, follow: [] }
  pure def resume_guard(t: Row, i: NoInput, c: Ctx): bool = true
  pure def resume_apply(t: Row, i: NoInput, c: Ctx): Move[Row, Do, Msg, Ask] = { next: { ...t, status: Ready }, follow: [] }
  // temper:generated begin
  // temper:generated end
}

module session_record {
  import temper.* from "./temper"
  import turn_record as turn_record
  import toolcall_record as toolcall_record
  import computer_record as computer_record
  import agent_record as agent_record

  type SessionId = Id
  type Status = Idle | Thinking | Calling | Acting | Connecting | WaitingApproval | Compacting | Done | Failed
  // What the model is sent each turn, provider-neutral: turn_runner writes it in the provider's own wire format.
  type Entry = { role: str, content: str, call_id: str, call_ids: List[str], call_names: List[str], call_args: List[str] }
  type Row = {
    status: Status,
    agent: agent_record::AgentId,
    title: str, soul: str, provider: str, model: str, tools: Set[str], approve: Set[str],
    history: List[Entry],
    turns: int,
    steps: int,
    // The model's tool calls not yet ended, in the order it asked; the head is the one being decided or run.
    queue_ids: List[str], queue_names: List[str], queue_args: List[str],
    asked_in: str,
    computer: str,
    computer_paused: bool,
    tokens_in: int, tokens_out: int,
    reply: str,
    why: str,
  }
  type Msg = NoMsgs
  type TurnIn = { provider: str, model: str, soul: str, max_tokens: int, computer: bool, unikon: bool, compact: bool, history: List[Entry] }
  type ComputerIn = { op: str, sandbox: str, paused: bool, call: str, args: str, unikon: bool }
  // One unikon tool call: its id and the model's input, {"tool": .., "arguments": {..}}.
  type UnikonIn = { call: str, args: str }
  type Ask = | AskModel(TurnIn) | AskComputer(ComputerIn) | AskUnikon(UnikonIn)
  // One model answer: its id, stop ("tool_use", "end", or why it failed), text, tool calls and token use.
  type TurnOut = { message: str, stop: str, text: str, call_ids: List[str], call_names: List[str], call_args: List[str], tokens_in: int, tokens_out: int }
  type ComputerOut = { sandbox: str, exit_code: int, stdout: str, stderr: str }
  type OpenIn = { title: str, soul: str, provider: str, model: str, tools: Set[str], approve: Set[str] }
  type AskIn = { text: str }
  type CallIn = { call: str }
  // The same three as an agent's: a move's generated cases name a sum's variants unqualified, so each record declares it.
  type Provider = OpenAI | Anthropic | OpenRouter
  type SwapIn = { provider: Provider, model: str }
  pure def provider_name(p: Provider): str = if (p == OpenAI) "openai" else if (p == Anthropic) "anthropic" else "openrouter"
  // One answer's token cap (room for a tool call that writes an app's files), model calls one ask may take (enough to
  // build, check and fix an app; a looping model still stops), and history entries kept before
  // the session asks the model to summarise them (so a request stays inside the provider's context window). Constants,
  // not an agent's inputs: each input multiplies the cases admission's search tries, 15,000 at most per move.
  pure val max_tokens: int = 16000
  pure val max_steps: int = 40
  pure val compact_after: int = 40

  pure val record: RecordDecl = { name: "Session", status: "status" }
  pure val moves: List[MoveDecl[Status]] = [
    { name: "open", from: FirstMove },
    { name: "ask", from: Statuses([Idle, Failed]) },
    { name: "answered_tools", from: Statuses([Thinking]) },
    { name: "answered", from: Statuses([Thinking]) },
    { name: "compacted", from: Statuses([Compacting]) },
    { name: "turn_refused", from: Statuses([Thinking, Compacting]) },
    { name: "turn_failed", from: Statuses([Thinking, Compacting]) },
    { name: "next_tool", from: Statuses([Calling]) },
    { name: "tool_done", from: Statuses([Acting]) },
    { name: "tool_done_again", from: Statuses([Acting]) },
    { name: "tool_refused", from: Statuses([Acting]) },
    { name: "tool_answered", from: Statuses([Connecting]) },
    { name: "tool_failed", from: Statuses([Acting, Connecting]) },
    { name: "approve", from: Statuses([WaitingApproval]) },
    { name: "deny", from: Statuses([WaitingApproval]) },
    { name: "swap_model", from: Statuses([Idle, Failed, WaitingApproval]) },
    { name: "pause_computer", from: Statuses([Idle]) },
    { name: "computer_paused", from: Statuses([Idle, Thinking, Calling, Acting, Connecting, WaitingApproval, Compacting, Failed]) },
    { name: "close", from: Statuses([Idle, Failed]) },
    { name: "computer_ended", from: Statuses([Done]) },
  ]
  // A computer idle 10 minutes is suspended. Reason: Tensorlake bills a running sandbox by the second, and a suspended one
  // keeps its disk, so the workspace persists and costs nothing while nobody uses it.
  pure val status_timers: List[StatusTimer[Status]] = [
    { name: "idle_computer", while_in: Idle, after: minutes(10), fires: "pause_computer" },
  ]
  pure val model_paths: ModulePaths = {
    applied: ["answered_tools", "answered", "turn_refused"], failed: ["turn_failed"], unknown: ["turn_failed"], refused: ["turn_failed"],
  }
  pure val compact_paths: ModulePaths = {
    applied: ["compacted", "turn_refused"], failed: ["turn_failed"], unknown: ["turn_failed"], refused: ["turn_failed"],
  }
  pure val exec_paths: ModulePaths = {
    applied: ["tool_done_again", "tool_done", "tool_refused"], failed: ["tool_failed"], unknown: ["tool_failed"], refused: ["tool_failed"],
  }
  pure val unikon_paths: ModulePaths = {
    applied: ["tool_answered"], failed: ["tool_failed"], unknown: ["tool_failed"], refused: ["tool_failed"],
  }
  pure val pause_paths: ModulePaths = { applied: ["computer_paused"], failed: [], unknown: [], refused: [] }
  pure val end_paths: ModulePaths = { applied: ["computer_ended"], failed: [], unknown: [], refused: [] }

  // ---- helpers over the session's own row
  pure def key_of(n: int): str = format("turn-{}", [as_text(n)])
  pure def rest_of(q: List[str]): List[str] = if (q.length() == 0) q else q.tail()
  pure def head_of(q: List[str]): str = if (q.length() == 0) "" else q.head()
  pure def ask_model(t: Row, history: List[Entry], compact: bool): Ask = AskModel({
    provider: t.provider, model: t.model, soul: t.soul, max_tokens: max_tokens, computer: t.tools.contains("bash"), unikon: t.tools.contains("unikon"),
    compact: compact, history: history,
  })
  // The spec, not the module, decides each call: a tool the agent holds no grant for is answered "refuse" by the module
  // with no door call, so the model is told why.
  pure def ask_computer(t: Row): Ask = AskComputer({
    op: if (t.tools.contains(head_of(t.queue_names))) "exec" else "refuse",
    sandbox: t.computer, paused: t.computer_paused, call: head_of(t.queue_ids), args: head_of(t.queue_args),
    // No command carries the owner's UniKon credential: an agent granted "unikon" calls the connector through its own
    // tool and unikon_door, so the key never reaches the computer (Rita, 27 Nov).
    unikon: false,
  })
  // A granted unikon call goes to unikon_door, with no computer; every other call (an ungranted one too, refused there)
  // to computer_door.
  pure def calls_unikon(t: Row): bool = head_of(t.queue_names) == "unikon" and t.tools.contains("unikon")
  pure def ask_unikon(t: Row): Ask = AskUnikon({ call: head_of(t.queue_ids), args: head_of(t.queue_args) })
  pure def said_entry(role: str, said: str): Entry = { role: role, content: said, call_id: "", call_ids: [], call_names: [], call_args: [] }
  pure def tool_entry(call: str, said: str): Entry = { role: "tool", content: said, call_id: call, call_ids: [], call_names: [], call_args: [] }
  // After the call at the head ends: the next call, else the next model turn, else stop at the agent's step limit.
  pure def after_call(t: Row): Status =
    if (rest_of(t.queue_ids).length() > 0) Calling else if (t.steps < max_steps) Thinking else Idle
  pure def why_after(place: Status): str = if (place == Idle) "Stopped at the agent's step limit." else ""
  pure def steps_after(t: Row, place: Status): int = if (place == Thinking) t.steps + 1 else t.steps

  pure def open_guard(i: OpenIn, c: Ctx): bool = require(by_message(c), "An agent opens a session.")
  pure def open_apply(i: OpenIn, c: Ctx): Move[Row, Do, Msg, Ask] = {
    next: {
      status: Idle, agent: sender(c), title: i.title, soul: i.soul, provider: i.provider, model: i.model,
      tools: i.tools, approve: i.approve, history: [], turns: 0, steps: 0,
      queue_ids: [], queue_names: [], queue_args: [], asked_in: "", computer: "", computer_paused: false, tokens_in: 0, tokens_out: 0,
      reply: "", why: "",
    },
    follow: [],
  }

  // ---- ask: one user turn, then the model.
  pure def ask_guard(v: View, i: AskIn, c: Ctx): bool = require(i.text != "", "An ask says something.")
  pure def ask_apply(v: View, i: AskIn, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: {
      ...v,
      turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
        status: turn_record::Written, parent: key_of(v.self.turns), role: "user", content: i.text, tool: "", model: "", tokens_in: 0, tokens_out: 0,
      }),
      self: {
        ...v.self, status: Thinking, steps: 1, turns: v.self.turns + 1, reply: "", why: "",
        history: v.self.history.append(said_entry("user", i.text)),
      },
    },
    follow: [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(said_entry("user", i.text)), false), paths: model_paths })],
  }

  // ---- the model's answer
  pure def answered_tools_guard(v: View, i: TurnOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only turn_runner's answer ends a turn."),
    require(i.stop == "tool_use" and i.call_ids.length() > 0, "The model asked for tools."),
    require(i.call_names.length() == i.call_ids.length() and i.call_args.length() == i.call_ids.length(), "Each call has a name and input."),
  }
  pure def answered_tools_apply(v: View, i: TurnOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: {
      ...v,
      turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
        status: turn_record::Written, parent: key_of(v.self.turns), role: "assistant", content: i.text, tool: i.message, model: v.self.model,
        tokens_in: i.tokens_in, tokens_out: i.tokens_out,
      }),
      self: {
        ...v.self, status: Calling, turns: v.self.turns + 1, asked_in: key_of(v.self.turns + 1),
        queue_ids: i.call_ids, queue_names: i.call_names, queue_args: i.call_args,
        history: v.self.history.append({ role: "assistant", content: i.text, call_id: "", call_ids: i.call_ids, call_names: i.call_names, call_args: i.call_args }),
        tokens_in: v.self.tokens_in + i.tokens_in, tokens_out: v.self.tokens_out + i.tokens_out,
      },
    },
    follow: [Intent(DoNext_tool(Nothing))],
  }
  pure def answered_guard(v: View, i: TurnOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only turn_runner's answer ends a turn."),
    require(i.stop == "end", "The model answered."),
  }
  pure def answered_apply(v: View, i: TurnOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    val place = if (v.self.history.length() + 1 > compact_after) Compacting else Idle
    {
      next: {
        ...v,
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "assistant", content: i.text, tool: i.message, model: v.self.model,
          tokens_in: i.tokens_in, tokens_out: i.tokens_out,
        }),
        self: {
          ...v.self, status: place, turns: v.self.turns + 1, reply: i.text, history: v.self.history.append(said_entry("assistant", i.text)),
          tokens_in: v.self.tokens_in + i.tokens_in, tokens_out: v.self.tokens_out + i.tokens_out,
        },
      },
      follow:
        if (place == Compacting) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(said_entry("assistant", i.text)), true), paths: compact_paths })]
        else [],
    }
  }
  // The summary replaces the history the model is sent; every turn before it stays in the session's Turns.
  pure def compacted_guard(v: View, i: TurnOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only turn_runner's answer compacts."),
    require(i.stop == "end" and i.text != "", "A compaction answers its summary."),
  }
  pure def compacted_apply(v: View, i: TurnOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: {
      ...v,
      turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
        status: turn_record::Written, parent: key_of(v.self.turns), role: "summary", content: i.text, tool: "", model: v.self.model,
        tokens_in: i.tokens_in, tokens_out: i.tokens_out,
      }),
      self: {
        ...v.self, status: Idle, turns: v.self.turns + 1,
        history: [said_entry("user", format("A summary of our conversation so far:\n{}", [i.text]))],
        tokens_in: v.self.tokens_in + i.tokens_in, tokens_out: v.self.tokens_out + i.tokens_out,
      },
    },
    follow: [],
  }
  // A provider's refusal or error status: the session fails with its reason, keeps its history, and may be asked again.
  pure def turn_refused_guard(v: View, i: TurnOut, c: Ctx): bool = require(by_module_answer(c), "Only turn_runner's answer ends a turn.")
  pure def turn_refused_apply(v: View, i: TurnOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: { ...v, self: { ...v.self, status: Failed, why: format("The model's turn ended: {} {}", [i.stop, i.text]) } }, follow: [],
  }
  // Failed or past its deadline: the provider may have answered, so nothing is retried (the destination is at most once).
  pure def turn_failed_guard(v: View, i: NoInput, c: Ctx): bool = require(by_module_answer(c), "Only turn_runner's end ends a turn.")
  pure def turn_failed_apply(v: View, i: NoInput, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: { ...v, self: { ...v.self, status: Failed, why: "The model's call failed or ended unknown." } }, follow: [],
  }

  // ---- next_tool: the head call becomes a ToolCall, waiting on a person or run (or refused) by computer_door.
  pure def next_tool_guard(v: View, i: NoInput, c: Ctx): bool = and {
    require(by_intent(c), "The session steps its own tool calls."),
    require(v.self.queue_ids.length() > 0, "A call is queued."),
  }
  pure def next_tool_apply(v: View, i: NoInput, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uname = head_of(v.self.queue_names)
    val waits = v.self.tools.contains(uname) and v.self.approve.contains(uname)
    val place = if (waits) WaitingApproval else if (calls_unikon(v.self)) Connecting else Acting
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.put(child_id(v.id, head_of(v.self.queue_ids)), {
          status: if (waits) toolcall_record::Waiting else toolcall_record::Running,
          turn: v.self.asked_in, tool: uname, args: head_of(v.self.queue_args), exit_code: -1, output: "",
        }),
        self: { ...v.self, status: place },
      },
      follow:
        if (place == Acting) [RunModule({ mod: "computer_door", input: ask_computer(v.self), paths: exec_paths })]
        else if (place == Connecting) [RunModule({ mod: "unikon_door", input: ask_unikon(v.self), paths: unikon_paths })]
        else [],
    }
  }
  pure def approve_guard(v: View, i: CallIn, c: Ctx): bool = and {
    require(is_person(c), "A person approves a tool call."),
    require(i.call == head_of(v.self.queue_ids), "It is the call the session waits on."),
  }
  pure def approve_apply(v: View, i: CallIn, c: Ctx): Move[View, Do, Msg, Ask] = {
    val place = if (calls_unikon(v.self)) Connecting else Acting
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, i.call), {
          ...v.toolcall_record_rows.get(child_id(v.id, i.call)), status: toolcall_record::Running,
        }),
        self: { ...v.self, status: place },
      },
      follow:
        if (place == Connecting) [RunModule({ mod: "unikon_door", input: ask_unikon(v.self), paths: unikon_paths })]
        else [RunModule({ mod: "computer_door", input: ask_computer(v.self), paths: exec_paths })],
    }
  }
  pure def deny_guard(v: View, i: CallIn, c: Ctx): bool = and {
    require(is_person(c), "A person denies a tool call."),
    require(i.call == head_of(v.self.queue_ids), "It is the call the session waits on."),
  }
  pure def deny_apply(v: View, i: CallIn, c: Ctx): Move[View, Do, Msg, Ask] = {
    val said = "A person denied this call."
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, i.call), {
          ...v.toolcall_record_rows.get(child_id(v.id, i.call)), status: toolcall_record::Denied, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: i.call, model: "", tokens_in: 0, tokens_out: 0,
        }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(i.call, said)),
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(i.call, said)), false), paths: model_paths })]
        else [],
    }
  }

  // ---- a call's end on the computer: its output is the tool's answer, and the loop goes on. The session's first call
  // on a sandbox makes its Computer (tool_done); every later call on it marks that Computer Ready (tool_done_again), since
  // a second put of the same child is refused as child_exists.
  pure def tool_done_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's answer ends a call."),
    require(v.self.queue_ids.length() > 0 and i.sandbox != "", "A call ran on a computer."),
    require(v.self.computer != i.sandbox, "The call ran on a new computer."),
  }
  pure def tool_done_again_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's answer ends a call."),
    require(v.self.queue_ids.length() > 0 and i.sandbox != "", "A call ran on a computer."),
    require(v.self.computer == i.sandbox, "The call ran on this session's computer."),
  }
  pure def tool_done_again_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uid = head_of(v.self.queue_ids)
    val said = format("exit {}\n{}{}", [as_text(i.exit_code), i.stdout, i.stderr])
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, uid), {
          ...v.toolcall_record_rows.get(child_id(v.id, uid)), status: toolcall_record::Done, exit_code: i.exit_code, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: uid, model: "", tokens_in: 0, tokens_out: 0,
        }),
        computer_record_rows: v.computer_record_rows.set(child_id(v.id, i.sandbox), { status: computer_record::Ready, sandbox: i.sandbox }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(uid, said)), computer: i.sandbox, computer_paused: false,
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(uid, said)), false), paths: model_paths })]
        else [],
    }
  }
  pure def tool_done_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uid = head_of(v.self.queue_ids)
    val said = format("exit {}\n{}{}", [as_text(i.exit_code), i.stdout, i.stderr])
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, uid), {
          ...v.toolcall_record_rows.get(child_id(v.id, uid)), status: toolcall_record::Done, exit_code: i.exit_code, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: uid, model: "", tokens_in: 0, tokens_out: 0,
        }),
        computer_record_rows: v.computer_record_rows.put(child_id(v.id, i.sandbox), { status: computer_record::Ready, sandbox: i.sandbox }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(uid, said)), computer: i.sandbox, computer_paused: false,
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(uid, said)), false), paths: model_paths })]
        else [],
    }
  }
  // A call the spec refused (no grant): computer_door answered its reason with no sandbox and no door call.
  pure def tool_refused_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's answer ends a call."),
    require(v.self.queue_ids.length() > 0, "A call is at the queue's head."),
  }
  pure def tool_refused_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uid = head_of(v.self.queue_ids)
    val said = format("exit {}\n{}{}", [as_text(i.exit_code), i.stdout, i.stderr])
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, uid), {
          ...v.toolcall_record_rows.get(child_id(v.id, uid)), status: toolcall_record::Denied, exit_code: i.exit_code, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: uid, model: "", tokens_in: 0, tokens_out: 0,
        }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(uid, said)),
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(uid, said)), false), paths: model_paths })]
        else [],
    }
  }
  // ---- a unikon call's end: unikon_door answered the connector's text with no computer, and the loop goes on.
  pure def tool_answered_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only unikon_door's answer ends a unikon call."),
    require(v.self.queue_ids.length() > 0 and i.sandbox == "", "A unikon call ran with no computer."),
  }
  pure def tool_answered_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uid = head_of(v.self.queue_ids)
    val said = format("exit {}\n{}{}", [as_text(i.exit_code), i.stdout, i.stderr])
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, uid), {
          ...v.toolcall_record_rows.get(child_id(v.id, uid)), status: toolcall_record::Done, exit_code: i.exit_code, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: uid, model: "", tokens_in: 0, tokens_out: 0,
        }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(uid, said)),
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(uid, said)), false), paths: model_paths })]
        else [],
    }
  }
  pure def tool_failed_guard(v: View, i: NoInput, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's end ends a call."),
    require(v.self.queue_ids.length() > 0, "A call is at the queue's head."),
  }
  pure def tool_failed_apply(v: View, i: NoInput, c: Ctx): Move[View, Do, Msg, Ask] = {
    val uid = head_of(v.self.queue_ids)
    val said = "This call could not run: its computer or the UniKon connector did not start or answer."
    val place = after_call(v.self)
    {
      next: {
        ...v,
        toolcall_record_rows: v.toolcall_record_rows.set(child_id(v.id, uid), {
          ...v.toolcall_record_rows.get(child_id(v.id, uid)), status: toolcall_record::Failed, output: said,
        }),
        turn_record_rows: v.turn_record_rows.put(child_id(v.id, key_of(v.self.turns + 1)), {
          status: turn_record::Written, parent: key_of(v.self.turns), role: "tool", content: said, tool: uid, model: "", tokens_in: 0, tokens_out: 0,
        }),
        self: {
          ...v.self, status: place, steps: steps_after(v.self, place), why: why_after(place), turns: v.self.turns + 1,
          queue_ids: rest_of(v.self.queue_ids), queue_names: rest_of(v.self.queue_names), queue_args: rest_of(v.self.queue_args),
          history: v.self.history.append(tool_entry(uid, said)),
        },
      },
      follow:
        if (place == Calling) [Intent(DoNext_tool(Nothing))]
        else if (place == Thinking) [RunModule({ mod: "turn_runner", input: ask_model(v.self, v.self.history.append(tool_entry(uid, said)), false), paths: model_paths })]
        else [],
    }
  }

  // ---- the model can be swapped mid-session: the history is provider-neutral, so the next turn goes to the new one.
  pure def swap_model_guard(v: View, i: SwapIn, c: Ctx): bool = and {
    require(i.model != "", "A model is named."),
  }
  pure def swap_model_apply(v: View, i: SwapIn, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: { ...v, self: { ...v.self, provider: provider_name(i.provider), model: i.model } }, follow: [],
  }

  // ---- the computer's idle pause and the session's close
  pure def pause_computer_guard(v: View, i: NoInput, c: Ctx): bool = and {
    require(by_timer(c), "The idle timer pauses a computer."),
    require(v.self.computer != "" and not(v.self.computer_paused), "A running computer is there to pause."),
  }
  pure def pause_computer_apply(v: View, i: NoInput, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: { ...v, self: v.self },
    follow: [RunModule({ mod: "computer_door", input: AskComputer({ op: "pause", sandbox: v.self.computer, paused: false, call: "", args: "", unikon: false }), paths: pause_paths })],
  }
  pure def computer_paused_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's answer pauses a computer."),
    require(i.exit_code == 0 and i.sandbox == v.self.computer and v.computer_record_rows.keys().contains(child_id(v.id, i.sandbox)), "This session's computer was suspended."),
  }
  pure def computer_paused_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: {
      ...v,
      computer_record_rows: v.computer_record_rows.set(child_id(v.id, i.sandbox), { ...v.computer_record_rows.get(child_id(v.id, i.sandbox)), status: computer_record::Paused }),
      self: { ...v.self, computer_paused: true },
    },
    follow: [],
  }
  pure def close_guard(v: View, i: NoInput, c: Ctx): bool = true
  pure def close_apply(v: View, i: NoInput, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: { ...v, self: { ...v.self, status: Done } },
    follow: [RunModule({ mod: "computer_door", input: AskComputer({ op: "end", sandbox: v.self.computer, paused: false, call: "", args: "", unikon: false }), paths: end_paths })],
  }
  pure def computer_ended_guard(v: View, i: ComputerOut, c: Ctx): bool = and {
    require(by_module_answer(c), "Only computer_door's answer ends a computer."),
    require(i.sandbox != "" and v.computer_record_rows.keys().contains(child_id(v.id, i.sandbox)), "This session's computer was deleted."),
  }
  pure def computer_ended_apply(v: View, i: ComputerOut, c: Ctx): Move[View, Do, Msg, Ask] = {
    next: {
      ...v,
      computer_record_rows: v.computer_record_rows.set(child_id(v.id, i.sandbox), { ...v.computer_record_rows.get(child_id(v.id, i.sandbox)), status: computer_record::Ended }),
      self: { ...v.self, computer: "" },
    },
    follow: [],
  }
  // temper:generated begin
  // temper:generated end
}

module temperpaw {
  import temper.* from "./temper"
  import turn_record as turn_record
  import toolcall_record as toolcall_record
  import computer_record as computer_record
  import memory_record as memory_record
  import catalog_record as catalog_record
  import session_record as session_record
  import agent_record as agent_record

  // Each module's contract, by value.
  type Entry = { role: str, content: str, call_id: str, call_ids: List[str], call_names: List[str], call_args: List[str] }
  type TurnIn = { provider: str, model: str, soul: str, max_tokens: int, computer: bool, unikon: bool, compact: bool, history: List[Entry] }
  type TurnOut = { message: str, stop: str, text: str, call_ids: List[str], call_names: List[str], call_args: List[str], tokens_in: int, tokens_out: int }
  type ComputerIn = { op: str, sandbox: str, paused: bool, call: str, args: str, unikon: bool }
  type ComputerOut = { sandbox: str, exit_code: int, stdout: str, stderr: str }
  type UnikonIn = { call: str, args: str }
  type CatalogIn = { source: str }
  type CatalogOut = { models: str, count: int }

  pure val doors: List[DoorDecl] = [
    // The model destinations. Deadline 300 s, the kernel's ceiling: one long answer can take minutes. At most once: a
    // retry would pay twice, so an unknown end fails the turn and a person asks again.
    DestinationDoor({ name: "openai", deadline: seconds(300), honours_keys: false, may_carry_personal_data: true, keeps_keys_for: None, status_query: None, at_most_once: true, reach: ReachEndpoint({ scheme: SecureHttp, host: "api.openai.com", port: 443, path_prefix: "/v1/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "authorization" }, { name: "prefix", value: "Bearer " }, { name: "header_key", value: "codex_api_key" }], keys: ["codex_api_key"] }),
    DestinationDoor({ name: "anthropic", deadline: seconds(300), honours_keys: false, may_carry_personal_data: true, keeps_keys_for: None, status_query: None, at_most_once: true, reach: ReachEndpoint({ scheme: SecureHttp, host: "api.anthropic.com", port: 443, path_prefix: "/v1/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "x-api-key" }, { name: "prefix", value: "" }, { name: "header_key", value: "anthropic_api_key" }], keys: ["anthropic_api_key"] }),
    DestinationDoor({ name: "openrouter", deadline: seconds(300), honours_keys: false, may_carry_personal_data: true, keeps_keys_for: None, status_query: None, at_most_once: true, reach: ReachEndpoint({ scheme: SecureHttp, host: "openrouter.ai", port: 443, path_prefix: "/api/v1/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "authorization" }, { name: "prefix", value: "Bearer " }, { name: "header_key", value: "openrouter_api_key" }], keys: ["openrouter_api_key"] }),
    // The computer: Tensorlake's sandbox API (make, suspend, resume, delete) and its process API at the one fixed host,
    // which names the sandbox by the x-tensorlake-sandbox-id header. A command runs at most 120 s, inside the 180 s deadline.
    DestinationDoor({ name: "tensorlake", deadline: seconds(90), honours_keys: true, may_carry_personal_data: false, keeps_keys_for: None, status_query: None, at_most_once: false, reach: ReachEndpoint({ scheme: SecureHttp, host: "api.tensorlake.ai", port: 443, path_prefix: "/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "authorization" }, { name: "prefix", value: "Bearer " }, { name: "header_key", value: "tensorlake_key" }], keys: ["tensorlake_key"] }),
    DestinationDoor({ name: "tensorlake_process", deadline: seconds(180), honours_keys: true, may_carry_personal_data: true, keeps_keys_for: None, status_query: None, at_most_once: true, reach: ReachEndpoint({ scheme: SecureHttp, host: "sandbox.tensorlake.ai", port: 443, path_prefix: "/api/v1/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "authorization" }, { name: "prefix", value: "Bearer " }, { name: "header_key", value: "tensorlake_process_key" }, ], keys: ["tensorlake_process_key"] }),
    // UniKon's connector, for the unikon tool: the owner's connector key goes in Authorization, put there by door_sign,
    // never on the computer. Deadline 300 s, the kernel's ceiling: an evolve can take minutes. At most once: a call may
    // make or move records, so an unknown end fails the call and the model is told.
    DestinationDoor({ name: "unikon", deadline: seconds(300), honours_keys: false, may_carry_personal_data: true, keeps_keys_for: None, status_query: None, at_most_once: true, reach: ReachEndpoint({ scheme: SecureHttp, host: "unikon.dev", port: 443, path_prefix: "/", streamed: None }), middleware: Some("door_sign"), params: [{ name: "header", value: "authorization" }, { name: "prefix", value: "Bearer " }, { name: "header_key", value: "unikon_mcp_token" }], keys: ["unikon_mcp_token"] }),
    // Keys live in the app's vault under the login names; each door puts its key in place, out of every module's sight.
    KeyDoor({ name: "codex_api_key" }),
    KeyDoor({ name: "anthropic_api_key" }),
    KeyDoor({ name: "openrouter_api_key" }),
    KeyDoor({ name: "tensorlake_key" }),
    KeyDoor({ name: "tensorlake_process_key" }),
    KeyDoor({ name: "unikon_mcp_token" }),
  ]
  pure val modules: List[ModuleDecl] = [
    { name: "turn_runner", source_hash: "@turn_runner.rs", form: CallingModule(["openai", "anthropic", "openrouter"]), input: "TurnIn", answer: "TurnOut", constraints: [] },
    { name: "computer_door", source_hash: "@computer_door.rs", form: CallingModule(["tensorlake", "tensorlake_process"]), input: "ComputerIn", answer: "ComputerOut", constraints: [] },
    { name: "unikon_door", source_hash: "@unikon_door.rs", form: CallingModule(["unikon"]), input: "UnikonIn", answer: "ComputerOut", constraints: [] },
    // OpenRouter's model list is public: the call carries no key.
    { name: "model_catalog", source_hash: "@model_catalog.rs", form: CallingModule(["openrouter"]), input: "CatalogIn", answer: "CatalogOut", constraints: [] },
    // Every door's middleware: it alone is handed the keys, and puts them in place at send time (S-DOOR-10).
    { name: "door_sign", source_hash: "@door_sign.rs", form: SigningModule, input: "", answer: "", constraints: [] },
  ]
  pure val roles: List[str] = ["owner", "member"]
  pure val statements: List[Statement] = [
    { decide: Permit, who: HasRole("owner"), what: ChangesOfApp(AppPropose), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ChangesOfApp(AppDeploy), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ChangesOfApp(AppGiveGrant), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ChangesOfApp(AppTakeGrant), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ChangesOfApp(AppWriteVaultKey), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOfApp(AppEventStream), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOfApp(AppEventStream), when: Always },
    // An agent's owner sets who it is and what it may do; a member may start sessions with it.
    { decide: Permit, who: HasRole("owner"), what: MovesOf({ record: "Agent", moves: ["create", "revise", "start_session", "pause", "resume"] }), when: Always },
    { decide: Permit, who: HasRole("member"), what: MovesOf({ record: "Agent", moves: ["start_session"] }), when: Always },
    // The loop's moves act for whoever asked (each is guarded on its cause, so no caller makes one directly).
    {
      decide: Permit, who: HasRole("owner"),
      what: MovesOf({ record: "Session", moves: [
        "open", "ask", "answered_tools", "answered", "compacted", "turn_refused", "turn_failed", "next_tool", "tool_done", "tool_done_again", "tool_refused",
        "tool_failed", "tool_answered", "approve", "deny", "swap_model", "pause_computer", "computer_paused", "close", "computer_ended",
      ] }),
      when: Always,
    },
    {
      decide: Permit, who: HasRole("member"),
      what: MovesOf({ record: "Session", moves: [
        "open", "ask", "answered_tools", "answered", "compacted", "turn_refused", "turn_failed", "next_tool", "tool_done", "tool_done_again", "tool_refused",
        "tool_failed", "tool_answered", "approve", "deny", "swap_model", "pause_computer", "computer_paused", "close", "computer_ended",
      ] }),
      when: Always,
    },
    // A status timer's fire and what it asks act as the app.
    { decide: Permit, who: CallerApp, what: MovesOf({ record: "Session", moves: ["pause_computer", "computer_paused"] }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: MovesOf({ record: "Memory", moves: ["remember", "revise", "archive", "restore"] }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: MovesOf({ record: "Catalog", moves: ["fetch", "refresh", "filled", "fetch_failed"] }), when: Always },
    { decide: Permit, who: HasRole("member"), what: MovesOf({ record: "Catalog", moves: ["fetch", "refresh", "filled", "fetch_failed"] }), when: Always },
    { decide: Permit, who: HasRole("member"), what: MovesOf({ record: "Memory", moves: ["remember", "revise", "archive", "restore"] }), when: Always },
    // No agent approves or denies its own tool calls. Agents may make and revise Agents, as the owner may (Rita,
    // 27 Nov: agents edit freely, approval can come later).
    { decide: Forbid, who: CallerAgent, what: MovesOf({ record: "Session", moves: ["approve", "deny"] }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Agent", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Agent", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Session", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Session", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Turn", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Turn", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "ToolCall", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "ToolCall", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Computer", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Computer", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Memory", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Memory", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("owner"), what: ReadsOf({ record: "Catalog", fields: AllFields }), when: Always },
    { decide: Permit, who: HasRole("member"), what: ReadsOf({ record: "Catalog", fields: AllFields }), when: Always },
  ]
  // temper:generated begin
  // temper:generated end
}
