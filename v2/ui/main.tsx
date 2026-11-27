import { start, useApp, useRecords, useRecord, add, move, can, ago, howItWorks } from "@katagami/app";
import { Routes, Route, Link, useParams, useNavigate } from "react-router";
import { useState, useEffect, useRef, useMemo } from "react";
import "./app.css";
import links from "./links.json";
import soul from "./soul.json";

type AnyRec = {
  id: string;
  place: string;
  title?: string;
  fields: Record<string, any>;
  moved_at?: string;
  created_at?: string;
  moved_by?: string;
  fresh?: boolean;
};

type Entry = {
  role: string;
  content: string;
  call_id?: string;
  call_ids?: string[];
  call_names?: string[];
  call_args?: string[];
};

const SUGGESTIONS = [
  {
    title: "Grow a business from a goal",
    subtitle: "Hard rules, judgment and deep work, organ by organ",
    prompt: "My goal: run a small online print shop. Grow it as one living app. System 0: an order ledger with a hard daily spend cap and a timer that flags any order unshipped after 48 hours. System 1: a choice point that sorts each new request into fixed actions for me to agree or overrule. System 2: deep work that drafts product listings. Start small, share the live link, and tell me which organ to grow next.",
  },
  {
    title: "Keep my bills under a hard cap",
    subtitle: "A ledger no move can overspend, with due-date timers",
    prompt: "My goal: never overspend on household bills. Grow an app with a ledger where each category has a hard monthly cap no move can pass, a timer that warns me 3 days before each bill is due, and a choice point that sorts each new expense into a category for me to agree or overrule. Share the live link and add my first categories.",
  },
  {
    title: "Run my hiring pipeline",
    subtitle: "Stages, deadlines and an offer only I sign off",
    prompt: "My goal: hire our first engineer. Grow an app where candidates move through fixed stages, an offer can't go out without my sign-off, a timer flags anyone waiting over 3 days for a reply, and a choice point suggests each candidate's next step from fixed options. Add deep work that drafts an interview scorecard per candidate, then share the live link.",
  },
  {
    title: "Grow yourself a Goals organ",
    subtitle: "Evolve your own spec, rules and screen live",
    prompt: "Grow yourself a new organ: a Goal record in your own spec that moves planned, growing, live, with an invariant that a goal can't be live without a linked app, and show my goals on your home screen. Evolve your spec and interface live, with no restart, then show me it working.",
  },
];

type CatalogEntry = { id: string; name: string; ctxK: number; price: string };

// The default first, then current, cheap and strong tool-calling models from the OpenRouter catalog (27 Nov 2026).
const FEATURED_OPENROUTER = [
  "anthropic/claude-sonnet-4.5",
  "google/gemini-3.8-flash",
  "google/gemini-4-hydrogen",
  "x-ai/grok-4.7",
  "anthropic/claude-haiku-5.5",
  "openai/gpt-6-luna",
  "deepseek/deepseek-v4.1-flash",
  "google/gemini-4-neon",
  "anthropic/claude-sonnet-5.5",
  "openai/gpt-6.1-terra",
  "qwen/qwen3.8-flash",
  "z-ai/glm-5.3-flash",
];

function formatPromptPrice(raw: string | undefined): string {
  const n = Number(raw);
  if (!Number.isFinite(n) || n < 0) return "";
  if (n === 0) return "free";
  const perM = n * 1_000_000;
  if (perM < 0.01) return "<$0.01/M";
  if (perM < 1) return `$${perM.toFixed(2)}/M`;
  return `$${perM % 1 === 0 ? perM.toFixed(0) : perM.toFixed(2)}/M`;
}

function parseCatalogModels(raw: unknown): CatalogEntry[] {
  const text = String(raw || "").trim();
  if (!text) return [];
  const out: CatalogEntry[] = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    const parts = line.split("\t");
    const id = (parts[0] || "").trim();
    if (!id) continue;
    const name = (parts[1] || id).trim();
    const ctxLen = Number(parts[2] || 0);
    const ctxK = ctxLen > 0 ? Math.round(ctxLen / 1000) : 0;
    const price = formatPromptPrice(parts[3]);
    out.push({ id, name, ctxK, price });
  }
  const rank = (id: string) => {
    const idx = FEATURED_OPENROUTER.indexOf(id);
    return idx >= 0 ? idx : 1000;
  };
  return out.sort((a, b) => rank(a.id) - rank(b.id) || a.id.localeCompare(b.id));
}

const DIRECT_MODELS: Record<string, CatalogEntry[]> = {
  openai: [
    { id: "gpt-6-luna", name: "OpenAI: GPT-6 Luna", ctxK: 1050, price: "" },
    { id: "gpt-6.1-terra", name: "OpenAI: GPT-6.1 Terra", ctxK: 1050, price: "" },
    { id: "gpt-6.2-sol", name: "OpenAI: GPT-6.2 Sol", ctxK: 1050, price: "" },
  ],
  anthropic: [
    { id: "claude-sonnet-4-5", name: "Anthropic: Claude Sonnet 4.5", ctxK: 200, price: "" },
    { id: "claude-opus-4-5", name: "Anthropic: Claude Opus 4.5", ctxK: 200, price: "" },
    { id: "claude-haiku-5-5", name: "Anthropic: Claude Haiku 5.5", ctxK: 1000, price: "" },
    { id: "claude-sonnet-5-5", name: "Anthropic: Claude Sonnet 5.5", ctxK: 1000, price: "" },
  ],
};

function formatCtx(ctxK: number): string {
  if (!ctxK || ctxK <= 0) return "";
  if (ctxK >= 1000) return `${(ctxK / 1000).toFixed(ctxK % 1000 === 0 ? 0 : 1)}M`;
  return `${ctxK}k`;
}

// The default agent's soul: v2/agent/soul.md with this app's id filled in.
const DEFAULT_SOUL: string = soul.soul;

function placeOf(r: AnyRec | null | undefined): string {
  return String(r?.place || "").toLowerCase().replace(/_/g, "");
}

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

function toProviderVariant(p: string): "OpenAI" | "Anthropic" | "OpenRouter" {
  const k = String(p || "").toLowerCase();
  if (k === "openai") return "OpenAI";
  if (k === "anthropic") return "Anthropic";
  return "OpenRouter";
}

function shortModel(model: string): string {
  const s = String(model || "claude-sonnet-4.5");
  const idx = s.lastIndexOf("/");
  return idx >= 0 ? s.slice(idx + 1) : s;
}

function deriveTitle(prompt: string): string {
  const clean = prompt.trim().replace(/\s+/g, " ");
  if (clean.length <= 44) return clean;
  const cut = clean.slice(0, 44);
  const sp = cut.lastIndexOf(" ");
  return (sp > 20 ? cut.slice(0, sp) : cut) + "…";
}

function commandOf(args: string): string {
  try {
    const a = JSON.parse(args);
    if (a && typeof a.command === "string") return a.command;
  } catch {}
  return String(args || "");
}

function describeToolCall(name: string, args: string): { kind: string; preview: string } {
  const cmd = commandOf(args).trim();
  const oneLine = cmd.replace(/\s+/g, " ");
  if (/^(unikon|katagami)\s+/i.test(oneLine) || /\b(construct_app|evolve_app|set_app_interface|inspect_app|use_app|find_app|search_ingredients|attach_app_dress)\b/.test(oneLine)) {
    const cleaned = oneLine.replace(/^(unikon|katagami)\s+/i, "");
    return { kind: "UniKon", preview: cleaned.slice(0, 88) };
  }
  const label = name === "bash" ? "Computer" : name || "Tool";
  return { kind: label, preview: oneLine.slice(0, 88) || "run" };
}

type AppLink = { slug: string; url: string; label: string };

function extractAppLinks(texts: string[]): AppLink[] {
  const seen = new Set<string>();
  const out: AppLink[] = [];
  const re = /https:\/\/(?:([a-z0-9]{8,20})\.unikon\.app|unikon\.dev\/([a-z0-9]{8,20}))(?:\/[^\s)\]"'<>]*)?/gi;
  for (const t of texts) {
    if (!t) continue;
    let m: RegExpExecArray | null;
    while ((m = re.exec(t)) !== null) {
      const slug = (m[1] || m[2] || "").toLowerCase();
      if (!slug || seen.has(slug)) continue;
      seen.add(slug);
      // Try to find a friendly title like `slug (Morning Rituals)` or `[Morning Rituals](url)`
      const mdMatch = t.match(new RegExp(`\\[([^\\]]{2,42})\\]\\(https:\\/\\/(?:${slug}\\.unikon\\.app|unikon\\.dev\\/${slug})`, "i"));
      const parenMatch = t.match(new RegExp(`${slug}\\s*\\(([^)]{2,42})\\)`, "i"));
      const friendly = (mdMatch?.[1] || parenMatch?.[1] || "").trim();
      out.push({
        slug,
        url: links.app.replace("{slug}", slug),
        label: friendly || `UniKon app · ${slug}`,
      });
    }
  }
  return out;
}

function shortAgo(ts: string | undefined): string {
  if (!ts) return "";
  const raw = ago(ts) || "";
  return raw
    .replace(/\s+ago$/i, "")
    .replace(/just now/i, "now")
    .replace(/\s*seconds?/i, "s")
    .replace(/\s*minutes?/i, "m")
    .replace(/\s*hours?/i, "h")
    .replace(/\s*days?/i, "d");
}

function BrandMark() {
  return (
    <svg className="tp-brand-mark" viewBox="0 0 24 24" aria-hidden="true">
      <rect x="2" y="2" width="20" height="20" rx="6" fill="var(--tp-accent-soft)" />
      <circle cx="9.5" cy="10.2" r="4.3" fill="var(--tp-accent)" fillOpacity="0.86" />
      <circle cx="14.7" cy="10.2" r="4.3" fill="var(--tp-pink)" fillOpacity="0.72" />
      <circle cx="12.1" cy="14.6" r="4.3" fill="var(--tp-ink)" fillOpacity="0.78" />
    </svg>
  );
}

function MenuIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
      <line x1="2.5" y1="4.5" x2="13.5" y2="4.5" />
      <line x1="2.5" y1="8" x2="13.5" y2="8" />
      <line x1="2.5" y1="11.5" x2="13.5" y2="11.5" />
    </svg>
  );
}

function SendIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <line x1="8" y1="13" x2="8" y2="3" />
      <polyline points="3.8,7.2 8,3 12.2,7.2" />
    </svg>
  );
}

function renderInline(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const tokenRe = /(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\(https?:\/\/[^\s)]+\)|https?:\/\/[^\s)<>]+)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = tokenRe.exec(text)) !== null) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    const tok = m[0];
    if (tok.startsWith("**") && tok.endsWith("**")) {
      nodes.push(<strong key={k++}>{tok.slice(2, -2)}</strong>);
    } else if (tok.startsWith("`") && tok.endsWith("`")) {
      nodes.push(<code key={k++}>{tok.slice(1, -1)}</code>);
    } else if (tok.startsWith("[")) {
      const lm = tok.match(/^\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)$/);
      if (lm) {
        nodes.push(
          <a key={k++} href={lm[2]} target="_blank" rel="noreferrer">
            {lm[1]}
          </a>,
        );
      } else {
        nodes.push(tok);
      }
    } else {
      const cleanUrl = tok.replace(/[.,;:!?]+$/, "");
      const trail = tok.slice(cleanUrl.length);
      nodes.push(
        <a key={k++} href={cleanUrl} target="_blank" rel="noreferrer">
          {cleanUrl}
        </a>,
      );
      if (trail) nodes.push(trail);
    }
    last = m.index + tok.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function MarkdownProse({ text }: { text: string }) {
  const blocks = useMemo(() => {
    const out: Array<{ kind: "code" | "heading" | "list" | "p"; lines: string[] }> = [];
    const rawLines = String(text || "").split("\n");
    let i = 0;
    while (i < rawLines.length) {
      const line = rawLines[i];
      if (line.trim().startsWith("```")) {
        const code: string[] = [];
        i++;
        while (i < rawLines.length && !rawLines[i].trim().startsWith("```")) {
          code.push(rawLines[i]);
          i++;
        }
        i++;
        out.push({ kind: "code", lines: code });
        continue;
      }
      if (!line.trim()) {
        i++;
        continue;
      }
      if (/^#{1,3}\s+/.test(line)) {
        out.push({ kind: "heading", lines: [line.replace(/^#{1,3}\s+/, "")] });
        i++;
        continue;
      }
      if (/^\s*[-*]\s+/.test(line)) {
        const items: string[] = [];
        while (i < rawLines.length && /^\s*[-*]\s+/.test(rawLines[i])) {
          items.push(rawLines[i].replace(/^\s*[-*]\s+/, ""));
          i++;
        }
        out.push({ kind: "list", lines: items });
        continue;
      }
      const para: string[] = [];
      while (
        i < rawLines.length &&
        rawLines[i].trim() &&
        !rawLines[i].trim().startsWith("```") &&
        !/^#{1,3}\s+/.test(rawLines[i]) &&
        !/^\s*[-*]\s+/.test(rawLines[i])
      ) {
        para.push(rawLines[i]);
        i++;
      }
      out.push({ kind: "p", lines: para });
    }
    return out;
  }, [text]);

  return (
    <div className="tp-assistant-body">
      {blocks.map((b, idx) => {
        if (b.kind === "code") {
          return (
            <pre key={idx}>
              <code>{b.lines.join("\n")}</code>
            </pre>
          );
        }
        if (b.kind === "heading") {
          return <h3 key={idx}>{renderInline(b.lines[0])}</h3>;
        }
        if (b.kind === "list") {
          return (
            <ul key={idx}>
              {b.lines.map((it, j) => (
                <li key={j}>{renderInline(it)}</li>
              ))}
            </ul>
          );
        }
        return <p key={idx}>{renderInline(b.lines.join(" "))}</p>;
      })}
    </div>
  );
}

function ModelModal({
  provider,
  model,
  catalog,
  saving,
  error,
  onSave,
  onClose,
}: {
  provider: string;
  model: string;
  catalog: AnyRec | null;
  saving: boolean;
  error: string;
  onSave: (provider: string, model: string) => void;
  onClose: () => void;
}) {
  const [prov, setProv] = useState(provider || "openrouter");
  const [mod, setMod] = useState(model || "anthropic/claude-sonnet-4.5");
  const [query, setQuery] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [catalogErr, setCatalogErr] = useState("");
  const autoFetchedRef = useRef(false);
  const searchRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  const catalogPlace = placeOf(catalog);
  const catalogLoading = refreshing || catalogPlace === "fetching";
  const liveOpenRouterModels = useMemo(() => parseCatalogModels(catalog?.fields?.models), [catalog?.fields?.models]);

  const triggerCatalogRefresh = async () => {
    if (catalogLoading) return;
    setRefreshing(true);
    setCatalogErr("");
    const res = catalog
      ? await move("catalog", catalog.id, "refresh", {})
      : await add("catalog", {});
    setRefreshing(false);
    if (!res.ok) setCatalogErr(res.reason || "Could not load OpenRouter catalog.");
  };

  // Auto-fetch live OpenRouter models once when opening the picker if no Catalog record exists yet.
  useEffect(() => {
    if (prov !== "openrouter" || catalog || autoFetchedRef.current) return;
    autoFetchedRef.current = true;
    void triggerCatalogRefresh();
  }, [prov, catalog]);

  const allForProvider = useMemo(() => {
    if (prov === "openrouter") return liveOpenRouterModels;
    return DIRECT_MODELS[prov] || [];
  }, [prov, liveOpenRouterModels]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return allForProvider.slice(0, 80);
    const tokens = q.split(/\s+/).filter(Boolean);
    const matches: CatalogEntry[] = [];
    for (const item of allForProvider) {
      const hay = `${item.id} ${item.name}`.toLowerCase();
      if (tokens.every((t) => hay.includes(t))) {
        matches.push(item);
        if (matches.length >= 80) break;
      }
    }
    return matches;
  }, [allForProvider, query]);

  const trimmedQuery = query.trim();
  const isCustomSlug = trimmedQuery.length > 0 && !/\s/.test(trimmedQuery);
  const hasExactMatch = useMemo(
    () => isCustomSlug && allForProvider.some((m) => m.id.toLowerCase() === trimmedQuery.toLowerCase()),
    [allForProvider, isCustomSlug, trimmedQuery],
  );

  const selectModel = (nextMod: string) => {
    setMod(nextMod);
  };

  const switchProvider = (nextProv: string) => {
    setProv(nextProv);
    setQuery("");
    if (nextProv === "openrouter" && !mod.includes("/")) {
      setMod("anthropic/claude-sonnet-4.5");
    } else if (nextProv === "openai" && mod.includes("/")) {
      setMod("gpt-6-luna");
    } else if (nextProv === "anthropic" && mod.includes("/")) {
      setMod("claude-sonnet-4-5");
    }
  };

  return (
    <div className="tp-modal-wrap" onClick={onClose}>
      <div className="tp-modal tp-modal-wide" role="dialog" aria-label="Model settings" onClick={(e) => e.stopPropagation()}>
        <div className="tp-modal-head">
          <h2>Choose model</h2>
          <div style={{ display: "flex", alignItems: "center", gap: "4px" }}>
            {prov === "openrouter" && (
              <button
                type="button"
                className="tp-ghost-btn"
                disabled={catalogLoading}
                onClick={() => void triggerCatalogRefresh()}
              >
                {catalogLoading ? "Refreshing…" : "Refresh"}
              </button>
            )}
            <button type="button" className="tp-ghost-btn" onClick={onClose}>
              Close
            </button>
          </div>
        </div>

        <div className="tp-seg" role="tablist" aria-label="Provider">
          <button
            type="button"
            role="tab"
            className="tp-seg-btn"
            aria-selected={prov === "openrouter"}
            data-active={prov === "openrouter" ? "true" : "false"}
            onClick={() => switchProvider("openrouter")}
          >
            OpenRouter
          </button>
          <button
            type="button"
            role="tab"
            className="tp-seg-btn"
            aria-selected={prov === "openai"}
            data-active={prov === "openai" ? "true" : "false"}
            onClick={() => switchProvider("openai")}
          >
            OpenAI
          </button>
          <button
            type="button"
            role="tab"
            className="tp-seg-btn"
            aria-selected={prov === "anthropic"}
            data-active={prov === "anthropic" ? "true" : "false"}
            onClick={() => switchProvider("anthropic")}
          >
            Anthropic
          </button>
        </div>

        <div className="tp-search-row">
          <input
            ref={searchRef}
            data-part="input"
            className="tp-input"
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && trimmedQuery && !saving) {
                e.preventDefault();
                const target = filtered[0]?.id || trimmedQuery;
                setMod(target);
                onSave(prov, target);
              }
            }}
            placeholder={
              prov === "openrouter"
                ? liveOpenRouterModels.length > 0
                  ? `Search ${liveOpenRouterModels.length} OpenRouter models or paste any slug…`
                  : "Search OpenRouter models or paste any slug…"
                : "Filter or type any model ID…"
            }
          />
        </div>

        {catalogErr && <div className="tp-error">{catalogErr}</div>}

        <ul className="tp-model-list" role="listbox" aria-label="Available models">
          {isCustomSlug && !hasExactMatch && (
            <li>
              <button
                type="button"
                className="tp-model-opt"
                data-selected={mod === trimmedQuery ? "true" : "false"}
                onClick={() => selectModel(trimmedQuery)}
              >
                <span className="tp-model-opt-main">
                  <span className="tp-model-opt-name">Use custom model ID</span>
                  <span className="tp-model-opt-id">{trimmedQuery}</span>
                </span>
                <span className="tp-model-opt-right">
                  {mod === trimmedQuery && <span className="tp-model-check">✓</span>}
                </span>
              </button>
            </li>
          )}

          {filtered.length === 0 ? (
            <li className="tp-model-empty">
              {prov === "openrouter" && catalogLoading
                ? "Loading live OpenRouter models…"
                : prov === "openrouter" && catalogPlace === "failed"
                  ? "Could not reach OpenRouter catalog. Click Refresh or paste any model slug above."
                  : trimmedQuery
                    ? "No matches — paste any exact model slug above."
                    : "Click Refresh to load live OpenRouter models, or paste any slug above."}
            </li>
          ) : (
            filtered.map((item) => {
              const selected = mod === item.id;
              const ctx = formatCtx(item.ctxK);
              return (
                <li key={item.id}>
                  <button
                    type="button"
                    className="tp-model-opt"
                    data-selected={selected ? "true" : "false"}
                    onClick={() => selectModel(item.id)}
                    onDoubleClick={() => {
                      setMod(item.id);
                      if (!saving) onSave(prov, item.id);
                    }}
                  >
                    <span className="tp-model-opt-main">
                      <span className="tp-model-opt-name">{item.name}</span>
                      <span className="tp-model-opt-id">{item.id}</span>
                    </span>
                    <span className="tp-model-opt-right">
                      {item.price && <span className="tp-model-ctx">{item.price}</span>}
                      {ctx && <span className="tp-model-ctx">{ctx}</span>}
                      {selected && <span className="tp-model-check">✓</span>}
                    </span>
                  </button>
                </li>
              );
            })
          )}
        </ul>

        {error && <div className="tp-error">{error}</div>}

        <div className="tp-foot-row">
          <span className="tp-model-opt-id" style={{ maxWidth: "66%" }}>
            {prov}/{mod}
          </span>
          <button
            type="button"
            className="tp-send-btn"
            disabled={saving || !mod.trim()}
            onClick={() => onSave(prov, mod.trim())}
          >
            {saving ? "Saving…" : "Apply"}
          </button>
        </div>
      </div>
    </div>
  );
}

function MemoryModal({ memories, onClose }: { memories: AnyRec[]; onClose: () => void }) {
  const active = memories.filter((m) => placeOf(m) !== "archived");
  return (
    <div className="tp-modal-wrap" onClick={onClose}>
      <div className="tp-modal" role="dialog" aria-label="Memory" onClick={(e) => e.stopPropagation()}>
        <div className="tp-modal-head">
          <h2>Memory ({active.length})</h2>
          <button type="button" className="tp-ghost-btn" onClick={onClose}>
            Close
          </button>
        </div>
        {active.length === 0 ? (
          <p className="tp-side-empty" style={{ padding: 0 }}>
            No saved memories yet. Ask TemperPaw to remember preferences or project context.
          </p>
        ) : (
          <ul className="tp-memory-list">
            {active.map((m) => (
              <li key={m.id} className="tp-memory-item">
                <span className="tp-memory-key">{m.fields.key || m.title || "note"}</span>
                <span className="tp-memory-val">{m.fields.content}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function Sidebar({
  appName,
  sessions,
  activeId,
  memoryCount,
  open,
  onClose,
  onOpenMemory,
}: {
  appName: string;
  sessions: AnyRec[];
  activeId: string;
  memoryCount: number;
  open: boolean;
  onClose: () => void;
  onOpenMemory: () => void;
}) {
  const navigate = useNavigate();

  const sorted = useMemo(() => {
    return [...sessions].sort((a, b) => {
      const ta = Date.parse(a.moved_at || a.created_at || "") || 0;
      const tb = Date.parse(b.moved_at || b.created_at || "") || 0;
      return tb - ta;
    });
  }, [sessions]);

  return (
    <>
      {open && <div className="tp-scrim" onClick={onClose} aria-hidden="true" />}
      <aside className="tp-sidebar" data-open={open ? "true" : "false"} aria-label="Sessions">
        <div className="tp-side-head">
          <button
            type="button"
            className="tp-brand"
            onClick={() => {
              navigate("/");
              onClose();
            }}
          >
            <BrandMark />
            <span>{appName || "TemperPaw"}</span>
          </button>
          <button
            type="button"
            className="tp-new-btn"
            onClick={() => {
              navigate("/");
              onClose();
            }}
          >
            + New
          </button>
        </div>

        <div className="tp-side-section">Sessions</div>
        <ul className="tp-session-list">
          {sorted.length === 0 ? (
            <li className="tp-side-empty">Start a prompt to open your first session.</li>
          ) : (
            sorted.map((s) => {
              const p = placeOf(s);
              const dotState =
                p === "thinking" || p === "calling" || p === "acting" || p === "compacting"
                  ? "active"
                  : p === "failed"
                    ? "failed"
                    : p === "waitingapproval"
                      ? "waiting"
                      : "idle";
              return (
                <li key={s.id}>
                  <Link
                    to={`/session/${s.id}`}
                    className="tp-session-item"
                    aria-current={s.id === activeId ? "page" : undefined}
                    onClick={onClose}
                  >
                    <span className="tp-session-dot" data-state={dotState} />
                    <span className="tp-session-title">{s.fields.title || "Untitled session"}</span>
                    <span className="tp-session-time">{shortAgo(s.moved_at || s.created_at)}</span>
                  </Link>
                </li>
              );
            })
          )}
        </ul>

        <div className="tp-side-foot">
          <div className="tp-foot-row">
            <button type="button" className="tp-ghost-btn" onClick={onOpenMemory}>
              Memory <span className="tp-badge-count">{memoryCount}</span>
            </button>
            <button type="button" className="tp-ghost-btn" onClick={howItWorks}>
              How it works
            </button>
          </div>
        </div>
      </aside>
    </>
  );
}

function ConversationStream({ session, toolCalls }: { session: AnyRec; toolCalls: AnyRec[] }) {
  const history = list<Entry>(session.fields.history);
  const place = placeOf(session);
  const qIds = list<string>(session.fields.queue_ids);
  const qNames = list<string>(session.fields.queue_names);
  const qArgs = list<string>(session.fields.queue_args);
  const headId = qIds[0];

  const [gateBusy, setGateBusy] = useState(false);
  const [gateErr, setGateErr] = useState("");

  // Map tool output by call_id so tool results fold cleanly into the assistant's collapsible chips.
  const toolOutputById = useMemo(() => {
    const map = new Map<string, { output: string; status: string }>();
    for (const tc of toolCalls) {
      const suffix = tc.id.includes(".") ? tc.id.slice(tc.id.lastIndexOf(".") + 1) : tc.id;
      map.set(suffix, { output: String(tc.fields.output || ""), status: placeOf(tc) });
    }
    for (const e of history) {
      if (e.role === "tool" && e.call_id) {
        const prev = map.get(e.call_id);
        map.set(e.call_id, {
          output: e.content || prev?.output || "",
          status: prev?.status || "done",
        });
      }
    }
    return map;
  }, [history, toolCalls]);

  const visibleEntries = useMemo(() => history.filter((e) => e.role !== "tool"), [history]);

  // Deduplicate app cards per user-prompt exchange and show them on the final assistant step of that exchange.
  const appLinksByEntryIdx = useMemo(() => {
    const result = new Map<number, AppLink[]>();
    let exchangeTexts: string[] = [];
    for (let idx = 0; idx < visibleEntries.length; idx++) {
      const e = visibleEntries[idx];
      if (e.role === "user") {
        exchangeTexts = [];
        continue;
      }
      if (e.content) exchangeTexts.push(e.content);
      for (const cid of e.call_ids || []) {
        const out = toolOutputById.get(cid);
        if (out?.output) exchangeTexts.push(out.output);
      }
      const next = visibleEntries[idx + 1];
      const isEndOfExchange = !next || next.role === "user";
      if (isEndOfExchange) {
        const links = extractAppLinks(exchangeTexts);
        if (links.length > 0) result.set(idx, links);
      }
    }
    return result;
  }, [visibleEntries, toolOutputById]);

  const decideGate = async (kind: "approve" | "deny") => {
    if (!headId || gateBusy) return;
    setGateBusy(true);
    setGateErr("");
    const res = await move("session", session.id, kind, { call: headId });
    setGateBusy(false);
    if (!res.ok) setGateErr(res.reason || "Could not submit decision.");
  };

  return (
    <div className="tp-stream">
      {visibleEntries.map((e, idx) => {
        if (e.role === "user") {
          return (
            <div key={idx} className="tp-msg" data-role="user">
              <div className="tp-user-bubble">{e.content}</div>
            </div>
          );
        }

        const cIds = e.call_ids || [];
        const cNames = e.call_names || [];
        const cArgs = e.call_args || [];
        const appLinks = appLinksByEntryIdx.get(idx) || [];

        return (
          <div key={idx} className="tp-msg" data-role="assistant">
            {e.content && <MarkdownProse text={e.content} />}

            {cIds.length > 0 && (
              <div className="tp-tools">
                {cIds.map((cid, k) => {
                  const tName = cNames[k] || "bash";
                  const tArg = cArgs[k] || "";
                  const { kind, preview } = describeToolCall(tName, tArg);
                  const res = toolOutputById.get(cid);
                  const st = res?.status || (headId === cid ? "running" : "done");
                  return (
                    <details key={cid} className="tp-tool-chip">
                      <summary>
                        <span className="tp-tool-icon" data-status={st === "failed" || st === "denied" ? "failed" : st === "done" ? "done" : "running"}>
                          {st === "failed" || st === "denied" ? "!" : st === "done" ? "✓" : "·"}
                        </span>
                        <span className="tp-tool-kind">{kind}</span>
                        <span className="tp-tool-cmd">{preview}</span>
                        <span className="tp-tool-chev">›</span>
                      </summary>
                      <div className="tp-tool-body">
                        <pre className="tp-tool-code">{commandOf(tArg)}</pre>
                        {res?.output && <pre className="tp-tool-code">{res.output}</pre>}
                      </div>
                    </details>
                  );
                })}
              </div>
            )}

            {appLinks.length > 0 && (
              <div className="tp-app-cards">
                {appLinks.map((a) => (
                  <a key={a.slug} href={a.url} target="_blank" rel="noreferrer" className="tp-app-card">
                    <span className="tp-app-glyph">UK</span>
                    <span className="tp-app-info">
                      <span className="tp-app-name">{a.label}</span>
                      <span className="tp-app-url">{a.slug}.unikon.app</span>
                    </span>
                    <span className="tp-app-open">Open ↗</span>
                  </a>
                ))}
              </div>
            )}
          </div>
        );
      })}

      {(place === "thinking" || place === "compacting") && (
        <div className="tp-working">
          <span className="tp-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <span>
            {place === "compacting" ? "Summarizing conversation…" : `Thinking with ${shortModel(session.fields.model)}…`}
          </span>
        </div>
      )}

      {(place === "calling" || place === "acting") && (
        <div className="tp-working">
          <span className="tp-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <span>
            {headId ? `${describeToolCall(qNames[0] || "bash", qArgs[0] || "").kind} running` : "Running tool…"}
          </span>
          {headId && <code>{describeToolCall(qNames[0] || "bash", qArgs[0] || "").preview}</code>}
        </div>
      )}

      {place === "waitingapproval" && headId && (
        <div className="tp-gate">
          <p className="tp-gate-title">Approve {qNames[0] || "command"}?</p>
          <pre className="tp-tool-code">{commandOf(qArgs[0] || "")}</pre>
          <div className="tp-gate-actions">
            <button type="button" className="tp-send-btn" disabled={gateBusy} onClick={() => decideGate("approve")}>
              Approve
            </button>
            <button type="button" className="tp-btn-secondary" disabled={gateBusy} onClick={() => decideGate("deny")}>
              Deny
            </button>
          </div>
          {gateErr && <div className="tp-error">{gateErr}</div>}
        </div>
      )}

      {session.fields.why && <div className="tp-error">{session.fields.why}</div>}
    </div>
  );
}

function TemperPawApp() {
  const { app } = useApp();
  const agents = useRecords("agent", { limit: 12 }) as AnyRec[];
  const sessions = useRecords("session", { limit: 50 }) as AnyRec[];
  const memories = useRecords("memory", { limit: 50 }) as AnyRec[];
  const catalogs = useRecords("catalog", { limit: 4 }) as AnyRec[];
  const catalog = catalogs[0] || null;

  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);

  // Auto-reload seamlessly when TemperPaw evolves its own interface via set_app_interface.
  const initialIfaceRef = useRef<string | null>(null);
  useEffect(() => {
    const iface = String((app as any)?.interface || "");
    if (!iface) return;
    if (initialIfaceRef.current === null) {
      initialIfaceRef.current = iface;
    } else if (initialIfaceRef.current !== iface) {
      initialIfaceRef.current = iface;
      window.location.reload();
    }
  }, [(app as any)?.interface]);

  // Default agent selection (prefer first Ready agent)
  const primaryAgent = useMemo(() => {
    return agents.find((a) => placeOf(a) === "ready") || agents[0] || null;
  }, [agents]);

  const [prefProvider, setPrefProvider] = useState("openrouter");
  const [prefModel, setPrefModel] = useState("anthropic/claude-sonnet-4.5");

  const activeProvider = primaryAgent?.fields?.provider || prefProvider;
  const activeModel = primaryAgent?.fields?.model || prefModel;

  const activeMemoryCount = useMemo(
    () => memories.filter((m) => placeOf(m) !== "archived").length,
    [memories],
  );

  return (
    <Routes>
      <Route
        path="/"
        element={
          <LandingScreen
            appName={app?.name || "TemperPaw"}
            agents={agents}
            primaryAgent={primaryAgent}
            sessions={sessions}
            memories={memories}
            catalog={catalog}
            memoryCount={activeMemoryCount}
            provider={activeProvider}
            model={activeModel}
            onUpdateDefaultModel={(p, m) => {
              setPrefProvider(p);
              setPrefModel(m);
            }}
            sidebarOpen={sidebarOpen}
            setSidebarOpen={setSidebarOpen}
            memoryOpen={memoryOpen}
            setMemoryOpen={setMemoryOpen}
          />
        }
      />
      <Route
        path="/session/:id"
        element={
          <SessionScreen
            appId={String(app?.id || "")}
            appName={app?.name || "TemperPaw"}
            sessions={sessions}
            memories={memories}
            catalog={catalog}
            memoryCount={activeMemoryCount}
            sidebarOpen={sidebarOpen}
            setSidebarOpen={setSidebarOpen}
            memoryOpen={memoryOpen}
            setMemoryOpen={setMemoryOpen}
          />
        }
      />
    </Routes>
  );
}

function LandingScreen({
  appName,
  agents,
  primaryAgent,
  sessions,
  memories,
  catalog,
  memoryCount,
  provider,
  model,
  onUpdateDefaultModel,
  sidebarOpen,
  setSidebarOpen,
  memoryOpen,
  setMemoryOpen,
}: {
  appName: string;
  agents: AnyRec[];
  primaryAgent: AnyRec | null;
  sessions: AnyRec[];
  memories: AnyRec[];
  catalog: AnyRec | null;
  memoryCount: number;
  provider: string;
  model: string;
  onUpdateDefaultModel: (p: string, m: string) => void;
  sidebarOpen: boolean;
  setSidebarOpen: (v: boolean) => void;
  memoryOpen: boolean;
  setMemoryOpen: (v: boolean) => void;
}) {
  const navigate = useNavigate();
  const [prompt, setPrompt] = useState("");
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState("");
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSaving, setModelSaving] = useState(false);
  const [modelErr, setModelErr] = useState("");
  const [pendingTick, setPendingTick] = useState(0);

  // Pending launch coordination: wait cleanly for newly spawned Agent or Session over live SSE.
  const pendingRef = useRef<{
    stage: "agent" | "session";
    prompt: string;
    knownAgentIds: Set<string>;
    knownSessionIds: Set<string>;
  } | null>(null);

  useEffect(() => {
    const p = pendingRef.current;
    if (!p) return;

    if (p.stage === "agent") {
      const created = agents.find((a) => !p.knownAgentIds.has(a.id) && placeOf(a) === "ready");
      if (!created) return;
      pendingRef.current = {
        stage: "session",
        prompt: p.prompt,
        knownAgentIds: p.knownAgentIds,
        knownSessionIds: new Set(sessions.map((s) => s.id)),
      };
      setPendingTick((n) => n + 1);
      void move("agent", created.id, "start_session", { title: deriveTitle(p.prompt) }).then((r) => {
        if (!r.ok) {
          pendingRef.current = null;
          setLaunching(false);
          setError(r.reason || "Could not start session.");
        } else {
          setPendingTick((n) => n + 1);
        }
      });
      return;
    }

    if (p.stage === "session") {
      const created = sessions.find((s) => !p.knownSessionIds.has(s.id));
      if (!created) return;
      const text = p.prompt;
      pendingRef.current = null;
      void move("session", created.id, "ask", { text }).then((r) => {
        setLaunching(false);
        if (!r.ok) setError(r.reason || "Session started, but could not send prompt.");
        navigate(`/session/${created.id}`);
      });
    }
  }, [agents, sessions, pendingTick, navigate]);

  const launch = async (rawText: string) => {
    const text = rawText.trim();
    if (!text || launching) return;
    setLaunching(true);
    setError("");
    setPrompt("");

    const knownSessionIds = new Set(sessions.map((s) => s.id));
    if (primaryAgent && placeOf(primaryAgent) === "ready") {
      pendingRef.current = {
        stage: "session",
        prompt: text,
        knownAgentIds: new Set(agents.map((a) => a.id)),
        knownSessionIds,
      };
      const res = await move("agent", primaryAgent.id, "start_session", { title: deriveTitle(text) });
      if (!res.ok) {
        pendingRef.current = null;
        setLaunching(false);
        setPrompt(text);
        setError(res.reason || "Could not start session.");
      } else {
        setPendingTick((n) => n + 1);
      }
      return;
    }

    // Zero-setup auto-create of the default TemperPaw agent with approve: [] (no popups!).
    const knownAgentIds = new Set(agents.map((a) => a.id));
    pendingRef.current = {
      stage: "agent",
      prompt: text,
      knownAgentIds,
      knownSessionIds,
    };
    const created = await add("agent", {
      name: "TemperPaw",
      soul: DEFAULT_SOUL,
      provider: toProviderVariant(provider),
      model,
      tools: ["bash", "unikon"],
      approve: [],
    });
    if (!created.ok) {
      pendingRef.current = null;
      setLaunching(false);
      setPrompt(text);
      setError(created.reason || "Could not initialize TemperPaw.");
      return;
    }
    if (created.id) {
      pendingRef.current = {
        stage: "session",
        prompt: text,
        knownAgentIds,
        knownSessionIds,
      };
      const started = await move("agent", created.id, "start_session", { title: deriveTitle(text) });
      if (!started.ok) {
        pendingRef.current = null;
        setLaunching(false);
        setPrompt(text);
        setError(started.reason || "Could not start session.");
      } else {
        setPendingTick((n) => n + 1);
      }
    } else {
      setPendingTick((n) => n + 1);
    }
  };

  const handleSaveModel = async (newProv: string, newMod: string) => {
    setModelErr("");
    onUpdateDefaultModel(newProv, newMod);
    if (!primaryAgent) {
      setModelOpen(false);
      return;
    }
    setModelSaving(true);
    const res = await move("agent", primaryAgent.id, "revise", {
      name: primaryAgent.fields.name || "TemperPaw",
      soul: primaryAgent.fields.soul || DEFAULT_SOUL,
      provider: toProviderVariant(newProv),
      model: newMod,
      tools: list<string>(primaryAgent.fields.tools).length ? list<string>(primaryAgent.fields.tools) : ["bash", "unikon"],
      approve: list<string>(primaryAgent.fields.approve),
    });
    setModelSaving(false);
    if (res.ok) {
      setModelOpen(false);
    } else {
      setModelErr(res.reason || "Could not update model.");
    }
  };

  return (
    <div className="tp-shell">
      <Sidebar
        appName={appName}
        sessions={sessions}
        activeId=""
        memoryCount={memoryCount}
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        onOpenMemory={() => {
          setSidebarOpen(false);
          setMemoryOpen(true);
        }}
      />

      <main className="tp-main">
        <header className="tp-topbar" data-bare="true">
          <div className="tp-topbar-left">
            <button
              type="button"
              className="tp-menu-btn"
              aria-label="Open sessions"
              onClick={() => setSidebarOpen(true)}
            >
              <MenuIcon />
            </button>
          </div>
          <div className="tp-topbar-right" />
        </header>

        <section className="tp-landing">
          <div className="tp-landing-inner">
            <div className="tp-hero">
              <div className="tp-hero-eyebrow">
                <span className="tp-pill-dot" />
                <span>TemperPaw · UniKon</span>
              </div>
              <h1>What should TemperPaw build next?</h1>
              <p>Hand it a goal. It grows a living app for it, with hard rules, judgment and deep work, and grows itself the same way.</p>
            </div>

            <form
              className="tp-composer"
              onSubmit={(e) => {
                e.preventDefault();
                void launch(prompt);
              }}
            >
              <textarea
                data-part="input"
                rows={2}
                value={prompt}
                disabled={launching}
                placeholder="Give TemperPaw a goal, or ask anything…"
                onChange={(e) => setPrompt(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void launch(prompt);
                  }
                }}
              />
              <div className="tp-composer-foot">
                <div className="tp-composer-meta">
                  <button type="button" className="tp-pill" onClick={() => setModelOpen(true)}>
                    <span>{shortModel(model)}</span>
                  </button>
                </div>
                <button type="submit" className="tp-send-btn" disabled={launching || !prompt.trim()}>
                  {launching ? "Starting…" : <>Send <SendIcon /></>}
                </button>
              </div>
            </form>

            {error && <div className="tp-error">{error}</div>}

            <div className="tp-chips" aria-label="Suggestions">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s.title}
                  type="button"
                  className="tp-chip"
                  disabled={launching}
                  onClick={() => void launch(s.prompt)}
                >
                  <div className="tp-chip-head">
                    <span className="tp-chip-title">{s.title}</span>
                    <span className="tp-chip-arrow" aria-hidden="true">→</span>
                  </div>
                  <span className="tp-chip-sub">{s.subtitle}</span>
                </button>
              ))}
            </div>
          </div>
        </section>
      </main>

      {modelOpen && (
        <ModelModal
          provider={provider}
          model={model}
          catalog={catalog}
          saving={modelSaving}
          error={modelErr}
          onSave={handleSaveModel}
          onClose={() => setModelOpen(false)}
        />
      )}

      {memoryOpen && <MemoryModal memories={memories} onClose={() => setMemoryOpen(false)} />}
    </div>
  );
}

function SessionScreen({
  appId,
  appName,
  sessions,
  memories,
  catalog,
  memoryCount,
  sidebarOpen,
  setSidebarOpen,
  memoryOpen,
  setMemoryOpen,
}: {
  appId: string;
  appName: string;
  sessions: AnyRec[];
  memories: AnyRec[];
  catalog: AnyRec | null;
  memoryCount: number;
  sidebarOpen: boolean;
  setSidebarOpen: (v: boolean) => void;
  memoryOpen: boolean;
  setMemoryOpen: (v: boolean) => void;
}) {
  const { id = "" } = useParams();
  const s = useRecord("session", id) as AnyRec | null;
  const allToolCalls = useRecords("toolcall", { limit: 120 }) as AnyRec[];

  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [sendErr, setSendErr] = useState("");
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSaving, setModelSaving] = useState(false);
  const [modelErr, setModelErr] = useState("");
  const endRef = useRef<HTMLDivElement | null>(null);

  const sessionToolCalls = useMemo(() => {
    if (!id) return [];
    return allToolCalls.filter((tc) => tc.id.startsWith(`${id}.`) || tc.id.startsWith(`${id}/`));
  }, [allToolCalls, id]);

  const place = placeOf(s);
  const isBusy = place === "thinking" || place === "calling" || place === "acting" || place === "compacting";
  const canAsk = !!s && (can(s, "ask") || place === "idle" || place === "failed");

  // If TemperPaw just ran set_app_interface on its own app in the latest settled turn, reload once to show the new UI.
  useEffect(() => {
    if (!s || !appId || place !== "idle") return;
    const turns = Number(s.fields.turns || 0);
    if (turns <= 0) return;
    const key = `tp-reloaded:${s.id}:${turns}`;
    try {
      if (window.sessionStorage.getItem(key)) return;
    } catch {
      return;
    }
    const hist = list<Entry>(s.fields.history);
    const tail = hist.slice(-4).map((e) => `${e.content || ""} ${(e.call_args || []).join(" ")}`).join("\n");
    const selfUpdated =
      /set_app_interface/i.test(tail) &&
      (tail.includes(appId) || /this\s+(page|app|interface|ui)/i.test(tail));
    if (!selfUpdated) return;
    try {
      window.sessionStorage.setItem(key, "1");
    } catch {}
    const timer = window.setTimeout(() => window.location.reload(), 900);
    return () => window.clearTimeout(timer);
  }, [s?.id, s?.fields?.turns, place, appId]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [s?.fields?.turns, place]);

  const send = async () => {
    const clean = text.trim();
    if (!s || !clean || sending || isBusy) return;
    setSending(true);
    setSendErr("");
    const res = await move("session", s.id, "ask", { text: clean });
    setSending(false);
    if (res.ok) {
      setText("");
    } else {
      setSendErr(res.reason || "Could not send message.");
    }
  };

  const handleSwapModel = async (prov: string, mod: string) => {
    if (!s) return;
    setModelSaving(true);
    setModelErr("");
    const res = await move("session", s.id, "swap_model", {
      provider: toProviderVariant(prov),
      model: mod,
    });
    setModelSaving(false);
    if (res.ok) {
      setModelOpen(false);
    } else {
      setModelErr(res.reason || "Swap is available when the session is idle.");
    }
  };

  const hasComputer = !!s?.fields?.computer;
  const pausedComputer = s?.fields?.computer_paused === true || s?.fields?.computer_paused === "true";

  return (
    <div className="tp-shell">
      <Sidebar
        appName={appName}
        sessions={sessions}
        activeId={id}
        memoryCount={memoryCount}
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        onOpenMemory={() => {
          setSidebarOpen(false);
          setMemoryOpen(true);
        }}
      />

      <main className="tp-main">
        <header className="tp-topbar">
          <div className="tp-topbar-left">
            <button
              type="button"
              className="tp-menu-btn"
              aria-label="Open sessions"
              onClick={() => setSidebarOpen(true)}
            >
              <MenuIcon />
            </button>
            <h1 className="tp-top-title">{s?.fields?.title || "Session"}</h1>
          </div>

          {s && (
            <div className="tp-topbar-right">
              {hasComputer && (
                <span className="tp-pill tp-sandbox-pill tp-hide-mobile">
                  <span className="tp-pill-dot" data-state={pausedComputer ? "paused" : isBusy ? "working" : "ready"} />
                  <span>{pausedComputer ? "Computer paused" : "Computer ready"}</span>
                </span>
              )}
              <button
                type="button"
                className="tp-pill"
                onClick={() => {
                  setModelErr("");
                  setModelOpen(true);
                }}
              >
                <span className="tp-pill-dot" data-state={isBusy ? "working" : "ready"} />
                <span>{shortModel(s.fields.model)}</span>
              </button>
            </div>
          )}
        </header>

        {!s ? (
          <div className="tp-landing">
            <p className="tp-side-empty">Loading session…</p>
          </div>
        ) : (
          <>
            <div className="tp-stream-wrap">
              <ConversationStream session={s} toolCalls={sessionToolCalls} />
              <div ref={endRef} />
            </div>

            <div className="tp-dock">
              <div className="tp-dock-inner">
                {sendErr && <div className="tp-error">{sendErr}</div>}
                <form
                  className="tp-composer"
                  onSubmit={(e) => {
                    e.preventDefault();
                    void send();
                  }}
                >
                  <textarea
                    data-part="input"
                    rows={1}
                    value={text}
                    disabled={!canAsk || isBusy || sending}
                    placeholder={
                      place === "done"
                        ? "This session is closed."
                        : isBusy
                          ? "TemperPaw is working…"
                          : "Reply or ask for a change…"
                    }
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.shiftKey) {
                        e.preventDefault();
                        void send();
                      }
                    }}
                  />
                  <div className="tp-composer-foot">
                    <div className="tp-composer-meta">
                      <span className="tp-session-time">
                        {Number(s.fields.tokens_in || 0) + Number(s.fields.tokens_out || 0) > 0
                          ? `${(Number(s.fields.tokens_in || 0) + Number(s.fields.tokens_out || 0)).toLocaleString()} tokens`
                          : "Enter to send"}
                      </span>
                    </div>
                    <button
                      type="submit"
                      className="tp-send-btn"
                      disabled={!canAsk || isBusy || sending || !text.trim()}
                    >
                      {isBusy ? "Working…" : <>Send <SendIcon /></>}
                    </button>
                  </div>
                </form>
              </div>
            </div>
          </>
        )}
      </main>

      {s && modelOpen && (
        <ModelModal
          provider={s.fields.provider}
          model={s.fields.model}
          catalog={catalog}
          saving={modelSaving}
          error={modelErr}
          onSave={handleSwapModel}
          onClose={() => setModelOpen(false)}
        />
      )}

      {memoryOpen && <MemoryModal memories={memories} onClose={() => setMemoryOpen(false)} />}
    </div>
  );
}

start(<TemperPawApp />);
