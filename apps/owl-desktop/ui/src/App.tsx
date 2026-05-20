import React, {
  useState, useRef, useEffect, useCallback, useMemo, KeyboardEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import hljs from "highlight.js";
import {
  BarChart, Bar, LineChart, Line, PieChart, Pie, Cell,
  XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer,
} from "recharts";

// ─── Types ────────────────────────────────────────────────────────────────────
interface ChatInput  { message: string; session_id?: string; }
interface ChatOutput { response: string; session_id: string; }
interface StreamChunk {
  id: string; text: string; thinking?: string;
  tool_call?: string; tool_args?: string; tool_result?: string;
  done: boolean; error?: string;
  input_tokens?:  number;
  output_tokens?: number;
}
interface ToolInvocation {
  name:    string;
  args?:   string;
  result?: string;
  /** Approval state when this tool requires user confirmation. */
  approval?: { call_id: string; status: "pending" | "approved" | "rejected"; reason?: string };
  /** Did this tool dispatch fail at execution time?  Drives the inline error pill. */
  error?:  string;
}
type Role = "user" | "assistant";
type AgentEvent =
  | { kind: "thought"; text: string }
  | { kind: "tool"; tool: ToolInvocation };

/** Multi-modal attachment staged in the composer before sending. */
interface AttachmentDraft {
  id:        string;
  kind:      "image" | "text";
  mime_type?: string;       // image only
  filename?:  string;       // text only
  /** Base64 payload (image) or raw text (text). */
  data:      string;
  /** Object URL for image preview thumbnails (revoked on remove). */
  preview?:  string;
  alt_text?: string;
}

/** Owl chibi pet — mirrors `owl-protocol::pet::Pet`. */
interface PetData {
  name:            string;
  level:           number;
  xp:              number;
  hunger:          number;
  happiness:       number;
  energy:          number;
  last_fed_ms:     number;
  last_active_ms:  number;
  tools_witnessed: number;
  last_say?:       string | null;
}
type PetMood = "happy" | "content" | "hungry" | "tired" | "sad" | "sleeping" | "excited";

/** Backend [`AgentEvent`] from `agent_event` channel.  Mirrors `owl-protocol::events::AgentEvent`. */
type BackendAgentEvent =
  | { type: "state_changed";          state: string }
  | { type: "text_chunk";              content: string }
  | { type: "tool_calling";            call: { name: string; args: any } }
  | { type: "tool_called";             result: { name: string; output: any; success: boolean; error?: string } }
  | { type: "tool_awaiting_approval";  call_id: string; call: { name: string; args: any }; reason?: string }
  | { type: "approval_resolved";       call_id: string; approved: boolean }
  | { type: "done";                    response: string }
  | { type: "error";                   message: string };

interface Message {
  id: number; role: Role; text: string;
  thinking?: string;
  thinkingMs?: number;
  thinkingTokens?: number;
  toolCalls?: ToolInvocation[];
  events?: AgentEvent[];
  streaming?: boolean;
  error?: string;
  workflowTraceId?: string;
  inputTokens?:  number;
  outputTokens?: number;
  attachments?: AttachmentDraft[];
  /** 👍 / 👎 user feedback; persisted for future analytics + insight bias. */
  reaction?: "up" | "down";
  ts: number;
}
interface Conversation {
  id: string;
  title: string;
  messages: Message[];
  ts: number;
  agent_id?: string;
}
type ToolSource = { type: "native" } | { type: "mcp"; server_id: string; server_name: string };
interface ToolInfo { name: string; description: string; source: ToolSource; }
type McpTransportKind = "stdio" | "web_socket" | "http";
interface McpServerConfig {
  id: string; name: string; transport: McpTransportKind;
  command?: string; args: string[]; env: string[];
  url?: string; enabled: boolean;
}
type SidePanel = "tools" | "agents" | "knowledge" | "settings" | null;

// ─── Orchestra DTOs ────────────────────────────────────────────────────────
interface AgentListItem {
  id: string; name: string; description: string;
  allowed_tools: string[]; skills: string[];
  can_spawn: boolean; max_steps: number; max_depth: number;
}
interface SkillListItem {
  id: string; name: string; description: string;
  trigger?: string | null;
  recommended_tools: string[];
}
interface WorkflowStepItem { id: string; agent: string; depends: string[]; }
interface WorkflowListItem {
  id: string; name: string; description: string;
  trigger?: string | null;
  steps: WorkflowStepItem[];
  on_failure: string;
  timeout_ms: number;
}
interface CommandListItem {
  id: string; name: string; description: string;
  kind: "text" | "workflow" | "tool";
  workflow_id?: string;
}

// ─── Workflow streaming ────────────────────────────────────────────────────
interface WorkflowStepResult {
  step_id: string; agent: string;
  status: "pending" | "running" | "completed" | "failed" | "skipped" | "cancelled";
  output: string; error?: string | null;
  attempts: number; started_at: number; finished_at: number;
}
interface WorkflowOutcome {
  trace_id: string; workflow_id: string; final_text: string;
  step_results: WorkflowStepResult[]; success: boolean;
}
type WorkflowEventInner =
  | { type: "workflow_started";   trace_id: string; workflow: string }
  | { type: "step_started";       trace_id: string; step: string; agent: string; prompt: string }
  | { type: "step_completed";     trace_id: string; step: string; result: WorkflowStepResult }
  | { type: "step_failed";        trace_id: string; step: string; error: string }
  | { type: "step_skipped";       trace_id: string; step: string; reason: string }
  | { type: "workflow_completed"; outcome: WorkflowOutcome }
  | { type: "workflow_cancelled"; trace_id: string; workflow: string };
interface WorkflowEventPayload { trace_id: string; event: WorkflowEventInner; }

interface AppSettings {
  gemini_api_key?: string; gemini_model?: string; gemini_embedding_model?: string;
  anthropic_api_key?: string;
  surreal_endpoint?: string; surreal_namespace?: string; surreal_database?: string;
  surreal_username?: string; surreal_password?: string;
}
interface SettingsView { settings: AppSettings; env_set: Record<keyof AppSettings, boolean>; path: string; }
interface WorkspaceInfo { path: string; name: string; id: string; exists: boolean; active: boolean; }
interface WorkspaceList { active: WorkspaceInfo | null; recent: WorkspaceInfo[]; }
interface IndexProgress { total: number; indexed: number; current: string; nodes: number; done: boolean; error?: string; }

// ─── Helpers ──────────────────────────────────────────────────────────────────
let _uid = 1;
const uid       = () => _uid++;
const newUuid   = () => crypto.randomUUID();
const titleFrom = (t: string) => t.length > 44 ? t.slice(0, 44) + "..." : t;
const formatK   = (n: number) => n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);

function tokensOf(m: Message): { input: number; output: number; estimated: boolean } {
  if (m.role === "assistant" && (m.inputTokens != null || m.outputTokens != null)) {
    return { input: m.inputTokens ?? 0, output: m.outputTokens ?? 0, estimated: false };
  }
  const text = m.text.length + (m.thinking?.length ?? 0);
  return m.role === "assistant"
    ? { input: 0, output: Math.round(text / 4), estimated: true }
    : { input: Math.round(text / 4), output: 0, estimated: true };
}

interface ConvUsage { input: number; output: number; estimated: boolean; }
function convUsage(c: Conversation): ConvUsage {
  const totals: ConvUsage = { input: 0, output: 0, estimated: false };
  let allReal = true;
  for (const m of c.messages) {
    const t = tokensOf(m);
    totals.input += t.input;
    totals.output += t.output;
    if (t.estimated) allReal = false;
  }
  totals.estimated = !allReal;
  return totals;
}

const COST_PER_M_INPUT  = 0.075;
const COST_PER_M_OUTPUT = 0.30;
function estCostUsd(input: number, output: number): number {
  return (input * COST_PER_M_INPUT + output * COST_PER_M_OUTPUT) / 1_000_000;
}
function formatCost(usd: number): string {
  if (usd <= 0) return "$0";
  if (usd < 0.01) return `<$0.01`;
  if (usd < 1) return `$${usd.toFixed(3)}`;
  return `$${usd.toFixed(2)}`;
}

function applyWorkflowEvent(msg: Message, event: WorkflowEventInner): Message {
  const events = [...(msg.events ?? [])];
  const findStep = (stepId: string): number => {
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.kind === "tool" && e.tool.name === stepId && !e.tool.result) return i;
    }
    return -1;
  };
  switch (event.type) {
    case "workflow_started":
      events.push({ kind: "thought", text: `Workflow **${event.workflow}** starting...` });
      return { ...msg, events };
    case "step_started":
      events.push({ kind: "tool", tool: {
        name: event.step,
        args: JSON.stringify({ agent: event.agent, prompt: event.prompt }, null, 2),
      }});
      return { ...msg, events };
    case "step_completed": {
      const idx = findStep(event.step);
      if (idx >= 0) {
        const r = event.result;
        events[idx] = { kind: "tool", tool: {
          name: event.step,
          args: (events[idx] as { kind: "tool"; tool: ToolInvocation }).tool.args,
          result: JSON.stringify({ agent: r.agent, status: r.status, attempts: r.attempts, output: r.output }, null, 2),
        }};
      }
      return { ...msg, events };
    }
    case "step_failed": {
      const idx = findStep(event.step);
      if (idx >= 0) {
        events[idx] = { kind: "tool", tool: {
          name: event.step,
          args: (events[idx] as { kind: "tool"; tool: ToolInvocation }).tool.args,
          result: JSON.stringify({ status: "failed", error: event.error }, null, 2),
        }};
      }
      return { ...msg, events, error: event.error };
    }
    case "step_skipped": {
      const idx = findStep(event.step);
      if (idx >= 0) {
        events[idx] = { kind: "tool", tool: {
          name: event.step,
          args: (events[idx] as { kind: "tool"; tool: ToolInvocation }).tool.args,
          result: JSON.stringify({ status: "skipped", reason: event.reason }, null, 2),
        }};
      }
      return { ...msg, events };
    }
    case "workflow_completed":
      return { ...msg, events, text: event.outcome.final_text, streaming: false, workflowTraceId: undefined };
    case "workflow_cancelled":
      return { ...msg, events, error: "Workflow cancelled", streaming: false, workflowTraceId: undefined };
  }
}

function convToMarkdown(c: Conversation): string {
  return `# ${c.title}\n\n` + c.messages.map(m =>
    `**${m.role === "user" ? "User" : "Assistant"}**\n\n${m.text}`
  ).join("\n\n---\n\n");
}

// ─── Design tokens ────────────────────────────────────────────────────────────
const C = {
  bg:       "#1a1a1f",
  sidebar:  "#111116",
  surface:  "#16161e",
  border:   "#28283a",
  text:     "#e0e0ec",
  muted:    "#6e6e85",
  accent:   "#cc785c",
  accentBg: "#cc785c18",
  userBg:   "#232330",
  inputBg:  "#1e1e28",
  code:     "#13131a",
  addBg:    "#0d2a0d",
  delBg:    "#2a0d0d",
  hunkBg:   "#0d1a2a",
};
const CHART_COLORS = ["#cc785c","#60a5fa","#4ade80","#f472b6","#fb923c","#a78bfa","#facc15"];

// ─── Chart detection ──────────────────────────────────────────────────────────
interface ChartSpec {
  type: "bar" | "line" | "pie";
  data: Array<Record<string, string | number>>;
  xKey?: string; yKey?: string; title?: string;
}
function tryParseChart(json: string): ChartSpec | null {
  try {
    const obj = JSON.parse(json);
    if (!obj || typeof obj !== "object") return null;
    if (!["bar", "line", "pie"].includes(obj.type)) return null;
    if (!Array.isArray(obj.data) || obj.data.length === 0) return null;
    return obj as ChartSpec;
  } catch { return null; }
}

// ─── CopyButton ───────────────────────────────────────────────────────────────
function CopyButton({ text, style }: { text: string; style?: React.CSSProperties }) {
  const [done, setDone] = useState(false);
  return (
    <button onClick={() => { navigator.clipboard.writeText(text).then(() => { setDone(true); setTimeout(() => setDone(false), 2000); }); }}
      style={{ ...sty.copyBtn, ...style }} title="Copy">
      {done ? "Copied" : "Copy"}
    </button>
  );
}

// ─── DiffBlock ────────────────────────────────────────────────────────────────
function DiffBlock({ code }: { code: string }) {
  return (
    <div style={{ margin: "10px 0", borderRadius: 9, overflow: "hidden", border: `1px solid ${C.border}` }}>
      {code.split("\n").map((line, i) => {
        const isAdd  = line.startsWith("+") && !line.startsWith("+++");
        const isDel  = line.startsWith("-") && !line.startsWith("---");
        const isHunk = line.startsWith("@@");
        const isHdr  = line.startsWith("---") || line.startsWith("+++") || line.startsWith("diff ") || line.startsWith("index ");
        const bg    = isAdd ? C.addBg : isDel ? C.delBg : isHunk ? C.hunkBg : isHdr ? "#1a1a2e" : "transparent";
        const color = isAdd ? "#4ade80" : isDel ? "#f87171" : isHunk ? "#60a5fa" : isHdr ? C.muted : C.text;
        return (
          <div key={i} style={{ background: bg, color, fontSize: 12.5, padding: "1px 14px", lineHeight: 1.6,
            fontFamily: "mono", whiteSpace: "pre" }}>
            {line || " "}
          </div>
        );
      })}
    </div>
  );
}

// ─── ChartBlock ───────────────────────────────────────────────────────────────
function ChartBlock({ data: spec }: { data: ChartSpec }) {
  const firstItem = spec.data[0] || {};
  const keys = Object.keys(firstItem);
  const xKey = spec.xKey || keys.find(k => typeof firstItem[k] === "string") || keys[0] || "name";
  const yKey = spec.yKey || keys.find(k => k !== xKey && typeof firstItem[k] === "number") || keys[1] || "value";
  const ttStyle = { background: C.surface, border: `1px solid ${C.border}`, borderRadius: 8, fontSize: 12 };
  return (
    <div style={{ margin: "12px 0", background: C.surface, border: `1px solid ${C.border}`, borderRadius: 10, padding: 16 }}>
      {spec.title && <p style={{ margin: "0 0 12px", fontSize: 13, fontWeight: 600, color: C.text }}>{spec.title}</p>}
      <ResponsiveContainer width="100%" height={240}>
        {spec.type === "bar" ? (
          <BarChart data={spec.data} margin={{ top: 4, right: 8, left: -16, bottom: 0 }}>
            <CartesianGrid strokeDasharray="3 3" stroke={C.border}/>
            <XAxis dataKey={xKey} tick={{ fill: C.muted, fontSize: 11 }}/>
            <YAxis tick={{ fill: C.muted, fontSize: 11 }}/>
            <Tooltip contentStyle={ttStyle}/>
            <Bar dataKey={yKey} fill={C.accent} radius={[4,4,0,0]}/>
          </BarChart>
        ) : spec.type === "line" ? (
          <LineChart data={spec.data} margin={{ top: 4, right: 8, left: -16, bottom: 0 }}>
            <CartesianGrid strokeDasharray="3 3" stroke={C.border}/>
            <XAxis dataKey={xKey} tick={{ fill: C.muted, fontSize: 11 }}/>
            <YAxis tick={{ fill: C.muted, fontSize: 11 }}/>
            <Tooltip contentStyle={ttStyle}/>
            <Line type="monotone" dataKey={yKey} stroke={C.accent} strokeWidth={2} dot={{ fill: C.accent, r: 3 }}/>
          </LineChart>
        ) : (
          <PieChart>
            <Pie data={spec.data} dataKey={yKey} nameKey={xKey} cx="50%" cy="50%" outerRadius={90} label>
              {spec.data.map((_e, i) => <Cell key={i} fill={CHART_COLORS[i % CHART_COLORS.length]}/>)}
            </Pie>
            <Tooltip contentStyle={ttStyle}/>
          </PieChart>
        )}
      </ResponsiveContainer>
    </div>
  );
}

// ─── SortableTable ────────────────────────────────────────────────────────────
function SortableTable({ children }: { children: React.ReactNode }) {
  const [sortCol, setSortCol] = useState(-1);
  const [sortAsc, setSortAsc] = useState(true);
  const childArr = React.Children.toArray(children) as React.ReactElement[];
  const thead = childArr.find((c: React.ReactElement) => (c.type as string) === "thead" ||
    (typeof c.type === "string" && c.type === "thead") ||
    (c.props && c.props.originalType === "thead") || c.props?.node?.tagName === "thead");
  const tbody = childArr.find((c: React.ReactElement) => (c.type as string) === "tbody" ||
    (typeof c.type === "string" && c.type === "tbody") || c.props?.node?.tagName === "tbody");
  const handleSort = (idx: number) => {
    if (sortCol === idx) setSortAsc(a => !a);
    else { setSortCol(idx); setSortAsc(true); }
  };
  const styledThead = thead ? React.cloneElement(thead as React.ReactElement, {},
    React.Children.map((thead as React.ReactElement).props.children, (row: React.ReactElement) =>
      React.cloneElement(row, {},
        React.Children.map(row.props.children, (th: React.ReactElement, i: number) =>
          <th key={i} style={{ ...sty.mdTh, cursor: "pointer", userSelect: "none" as const,
            background: sortCol === i ? "#1e1e2e" : "#1c1c26" }}
            onClick={() => handleSort(i)}>
            <span style={{ display: "flex", alignItems: "center", gap: 4, justifyContent: "space-between" }}>
              <span>{th.props.children}</span>
              <span style={{ opacity: sortCol === i ? 1 : 0.3, fontSize: 10 }}>
                {sortCol === i ? (sortAsc ? "^" : "v") : "="}
              </span>
            </span>
          </th>
        )
      )
    )
  ) : thead;
  let styledTbody = tbody;
  if (tbody && sortCol >= 0) {
    const tbodyEl = tbody as React.ReactElement;
    const rows = React.Children.toArray(tbodyEl.props.children) as React.ReactElement[];
    const sorted = [...rows].sort((a, b) => {
      const getCells = (r: React.ReactElement) => React.Children.toArray(r.props.children) as React.ReactElement[];
      const aVal = String(getCells(a)[sortCol]?.props?.children ?? "");
      const bVal = String(getCells(b)[sortCol]?.props?.children ?? "");
      const n = parseFloat(aVal) - parseFloat(bVal);
      const cmp = isNaN(n) ? aVal.localeCompare(bVal) : n;
      return sortAsc ? cmp : -cmp;
    });
    const styledRows = sorted.map((r: React.ReactElement, ri: number) =>
      React.cloneElement(r, { style: { background: ri % 2 === 0 ? "transparent" : "#ffffff05",
        borderBottom: `1px solid ${C.border}22` } })
    );
    styledTbody = React.cloneElement(tbodyEl, {}, styledRows);
  }
  return (
    <div style={{ overflowX: "auto", margin: "12px 0", borderRadius: 9,
      border: `1px solid ${C.border}`, overflow: "auto" }}>
      <table style={{ ...sty.mdTable, borderRadius: 0 }}>
        {styledThead}
        {styledTbody}
      </table>
    </div>
  );
}

// ─── CodeBlock ────────────────────────────────────────────────────────────────
const CODE_COLLAPSE = 30;
function CodeBlock({ lang, codeStr }: { lang?: string; codeStr: string }) {
  const lines = useMemo(() => codeStr.split("\n"), [codeStr]);
  const isLong = lines.length > CODE_COLLAPSE;
  const [expanded, setExpanded] = useState(false);
  const visibleCode = isLong && !expanded ? lines.slice(0, CODE_COLLAPSE).join("\n") : codeStr;
  const highlighted = useMemo(() => {
    if (lang && hljs.getLanguage(lang)) {
      return hljs.highlight(visibleCode, { language: lang, ignoreIllegals: true }).value;
    } else if (visibleCode.length < 4000) {
      return hljs.highlightAuto(visibleCode).value;
    }
    return visibleCode.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;");
  }, [lang, visibleCode]);
  const visibleCount = isLong && !expanded ? CODE_COLLAPSE : lines.length;
  const lineNumbers = Array.from({ length: visibleCount }, (_, i) => i + 1).join("\n");
  return (
    <div style={{ position: "relative", margin: "10px 0" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center",
        background: "#0d0d14", borderRadius: "9px 9px 0 0", padding: "5px 12px",
        border: `1px solid ${C.border}`, borderBottom: "none" }}>
        <span style={{ fontSize: 10.5, color: C.muted, fontFamily: "monospace" }}>{lang || "text"}</span>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ fontSize: 10, color: C.muted }}>{lines.length} lines</span>
          <CopyButton text={codeStr}/>
        </div>
      </div>
      <div style={{ display: "flex", background: C.code, borderRadius: "0 0 9px 9px",
        border: `1px solid ${C.border}`, borderTop: "none", overflowX: "auto" }}>
        <pre style={{ margin: 0, padding: "14px 10px", lineHeight: 1.6, fontSize: 12.5,
          fontFamily: "mono", color: "#3a3a58", textAlign: "right" as const, userSelect: "none" as const,
          borderRight: `1px solid ${C.border}40`, background: "#0b0b12", flexShrink: 0, minWidth: 40 }}>
          {lineNumbers}
        </pre>
        <pre style={{ margin: 0, padding: "14px 16px", lineHeight: 1.6, flex: 1, overflowX: "visible" as const, minWidth: 0 }}>
          <code dangerouslySetInnerHTML={{ __html: highlighted }} style={sty.codeInner}/>
        </pre>
      </div>
      {isLong && (
        <button onClick={() => setExpanded(e => !e)} style={sty.collapseBtn}>
          {expanded ? "Collapse" : `Show ${lines.length - CODE_COLLAPSE} more lines`}
        </button>
      )}
    </div>
  );
}

// ─── MarkdownMessage ──────────────────────────────────────────────────────────
function MarkdownMessage({ text, streaming }: { text: string; streaming?: boolean }) {
  const content = text;
  return (
    <div style={{ fontSize: 14, lineHeight: 1.8, color: C.text }}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}
        components={{
          pre({ children, ...rest }) {
            const child = React.Children.toArray(children)[0] as React.ReactElement | undefined;
            if ((child?.props as Record<string, unknown>)?.["data-block"] === "1") return <>{children}</>;
            return <pre {...rest as React.HTMLAttributes<HTMLPreElement>}>{children}</pre>;
          },
          code({ className, children }) {
            const lang = /language-(\w+)/.exec(className || "")?.[1];
            const raw = String(children ?? "");
            const codeStr = raw.replace(/\n$/, "");
            const isBlock = !!lang || codeStr.includes("\n");
            if (!isBlock) return <code style={sty.inlineCode}>{children}</code>;
            if (lang === "diff" || lang === "patch") return <div data-block="1"><DiffBlock code={codeStr}/></div>;
            if (lang === "json" || lang === "chart") {
              const chart = tryParseChart(codeStr);
              if (chart) return <div data-block="1"><ChartBlock data={chart}/></div>;
            }
            return <div data-block="1"><CodeBlock lang={lang} codeStr={codeStr}/></div>;
          },
          table({ children }) { return <SortableTable>{children}</SortableTable>; },
          thead({ children }) { return <thead style={{ borderBottom: `2px solid ${C.border}` }}>{children}</thead>; },
          th({ children }) { return <th style={sty.mdTh}>{children}</th>; },
          td({ children }) { return <td style={sty.mdTd}>{children}</td>; },
          tr({ children, ...rest }) { return <tr style={{ borderBottom: `1px solid ${C.border}22` }} {...rest as React.HTMLAttributes<HTMLTableRowElement>}>{children}</tr>; },
          input({ type, checked }) {
            if (type === "checkbox") {
              return <span style={{ display: "inline-block", width: 14, height: 14, borderRadius: 3,
                verticalAlign: "middle", marginRight: 5,
                border: `2px solid ${checked ? C.accent : C.border}`,
                background: checked ? C.accent : "transparent", position: "relative" as const }}>
                {checked && <span style={{ position: "absolute" as const, top: -3, left: 1, color: "#fff", fontSize: 11, fontWeight: 700 }}>+</span>}
              </span>;
            }
            return <input type={type} checked={checked} readOnly/>;
          },
          blockquote({ children }) { return <blockquote style={sty.mdBlockquote}>{children}</blockquote>; },
          h1({ children }) { return <h1 style={{ ...sty.mdH, fontSize: 20, marginTop: 22 }}>{children}</h1>; },
          h2({ children }) { return <h2 style={{ ...sty.mdH, fontSize: 17, marginTop: 18 }}>{children}</h2>; },
          h3({ children }) { return <h3 style={{ ...sty.mdH, fontSize: 15, marginTop: 14 }}>{children}</h3>; },
          a({ href, children }) { return <a href={href} style={{ color: C.accent, textDecoration: "underline", textDecorationColor: `${C.accent}60` }} target="_blank" rel="noopener noreferrer">{children}</a>; },
          ul({ children }) { return <ul style={sty.mdList}>{children}</ul>; },
          ol({ children }) { return <ol style={{ ...sty.mdList, listStyleType: "decimal" }}>{children}</ol>; },
          li({ children }) { return <li style={{ marginBottom: 3 }}>{children}</li>; },
          p({ children }) { return <p style={{ margin: "0 0 10px", lineHeight: 1.8 }}>{children}</p>; },
          hr() { return <hr style={{ border: "none", borderTop: `1px solid ${C.border}`, margin: "16px 0" }}/>; },
          strong({ children }) { return <strong style={{ fontWeight: 700, color: C.text }}>{children}</strong>; },
        }}>
        {content}
      </ReactMarkdown>
      {streaming && text && <span aria-hidden="true" style={sty.streamingCursor}/>}
    </div>
  );
}

// ─── ActivityPanel ────────────────────────────────────────────────────────────
function ActivityPanel({ events, streaming, durationMs, tokenCount, onResolveApproval, pendingApprovals }:
  { events: AgentEvent[]; streaming?: boolean; durationMs?: number; tokenCount?: number;
    onResolveApproval?: (callId: string, approved: boolean) => void;
    /** Tools currently waiting on the user — surfaced even when the panel is collapsed. */
    pendingApprovals?: ToolInvocation[];
  }) {
  // Auto-expand when the agent is paused waiting for approval — the user
  // can't dismiss a confirmation dialog they can't see.
  const [open, setOpen] = useState(false);
  const hasPending = (pendingApprovals?.length ?? 0) > 0;
  useEffect(() => { if (hasPending) setOpen(true); }, [hasPending]);

  if (events.length === 0 && !hasPending) return null;
  const toolCount = events.filter(e => e.kind === "tool").length;
  const thoughtChars = events.reduce((n, e) => n + (e.kind === "thought" ? e.text.length : 0), 0);
  const durationStr = durationMs != null ? `${(durationMs / 1000).toFixed(1)}s` : null;
  const tokenStr = tokenCount != null ? `~${formatK(tokenCount)} tokens`
    : thoughtChars > 0 ? `~${formatK(Math.round(thoughtChars / 4))} tokens` : null;
  const summary = [
    streaming ? "Thinking..." : durationStr ? `Thought for ${durationStr}` : "Thinking",
    tokenStr,
    toolCount > 0 ? `${toolCount} tool${toolCount > 1 ? "s" : ""}` : null,
    hasPending ? `⚠ ${pendingApprovals!.length} need approval` : null,
  ].filter(Boolean).join(" · ");
  return (
    <div style={sty.thinkingWrap}>
      <button style={sty.thinkingToggle} onClick={() => setOpen(o => !o)}>
        <span style={{ fontSize: 12, display: "flex", alignItems: "center", gap: 6 }}>
          <span style={{ color: hasPending ? "#fbbf24" : C.accent }}>{summary}</span>
        </span>
        <IcChevron open={open}/>
      </button>
      {open && (
        <div style={sty.thinkingBody}>
          <div style={{ padding: "10px 14px", display: "flex", flexDirection: "column", gap: 8 }}>
            {events.map((e, i) => {
              if (e.kind === "thought") return <ThoughtBlock key={i} text={e.text}/>;
              if (e.tool.approval?.status === "pending" && onResolveApproval) {
                return (
                  <ApprovalCard
                    key={`${e.tool.approval.call_id}-${i}`}
                    tool={e.tool}
                    onResolve={a => onResolveApproval(e.tool.approval!.call_id, a)}
                  />
                );
              }
              return FILE_MUTATION_TOOLS.has(e.tool.name)
                ? <FileChangeCard key={i} tool={e.tool}/>
                : <ToolChip key={i} tool={e.tool}/>;
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function ThoughtBlock({ text }: { text: string }) {
  return (
    <div style={{ fontSize: 12.5, color: "#8a8aa8", lineHeight: 1.7,
      borderLeft: `2px solid ${C.border}`, paddingLeft: 10 }}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}
        components={{
          p({ children }) { return <p style={{ margin: "0 0 4px" }}>{children}</p>; },
          code({ children }) { return <code style={{ background: "#1a1a28", borderRadius: 3,
            padding: "1px 4px", fontSize: 11.5, fontFamily: "monospace", color: "#a0a0c8" }}>{children}</code>; },
        }}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

// ─── TypingDots & ActivityIndicator ──────────────────────────────────────────
const TypingDots = () => (
  <span style={{ display: "inline-flex", gap: 5, alignItems: "center", padding: "6px 0" }}>
    {[0,1,2].map(i => (
      <span key={i} style={{ width: 6, height: 6, borderRadius: "50%", background: C.muted,
        display: "inline-block", animation: "pulse 1.2s ease-in-out infinite", animationDelay: `${i*0.18}s` }}/>
    ))}
  </span>
);

// ─── Time formatting ──────────────────────────────────────────────────────────
function formatTimeAgo(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return new Date(ts).toLocaleDateString();
}
function formatTimeAbsolute(ts: number): string {
  return new Date(ts).toLocaleString();
}

// ─── Workflow step detection ──────────────────────────────────────────────────
interface WorkflowStepMeta {
  status: "completed" | "failed" | "skipped" | "running" | string;
  agent?: string; output?: string; error?: string; reason?: string;
  attempts?: number;
}
function parseWorkflowStep(tool: ToolInvocation): WorkflowStepMeta | null {
  if (!tool.result) {
    if (tool.args) {
      try {
        const a = JSON.parse(tool.args);
        if (a && typeof a === "object" && "agent" in a && "prompt" in a) return { status: "running", agent: a.agent };
      } catch { /* ignore */ }
    }
    return null;
  }
  try {
    const r = JSON.parse(tool.result);
    if (r && typeof r === "object" && "status" in r) return r as WorkflowStepMeta;
  } catch { /* not workflow-shaped */ }
  return null;
}

// ─── Tool call chip ───────────────────────────────────────────────────────────
function ToolChip({ tool }: { tool: ToolInvocation }) {
  const [open, setOpen] = useState(false);
  const args   = useMemo(() => tryFormatJson(tool.args),   [tool.args]);
  const result = useMemo(() => tryFormatJson(tool.result), [tool.result]);
  const wf     = useMemo(() => parseWorkflowStep(tool),    [tool]);
  const statusColor =
    !wf ? "#60a5fa"
    : wf.status === "completed" ? "#4ade80"
    : wf.status === "failed" ? "#f87171"
    : wf.status === "skipped" ? "#a78bfa"
    : wf.status === "running" ? "#60a5fa" : "#60a5fa";
  return (
    <div style={{ display: "flex", flexDirection: "column" as const }}>
      <button style={{ ...sty.toolChip, borderColor: `${statusColor}30` }} onClick={() => setOpen(o => !o)}>
        <span style={{ width: 6, height: 6, borderRadius: "50%", background: statusColor, flexShrink: 0,
          animation: wf?.status === "running" ? "pulse 1.2s ease-in-out infinite" : undefined }}/>
        <span style={{ fontFamily: "mono", color: statusColor }}>{tool.name}</span>
        {wf?.agent && <span style={{ fontSize: 10.5, color: C.muted }}>{"→"} {wf.agent}</span>}
        <IcChevron open={open}/>
      </button>
      {open && (
        <div style={sty.toolDetail}>
          {tool.args && <>
            <div style={sty.toolDetailLabel}>args</div>
            <pre style={sty.toolDetailPre}>{args}</pre>
          </>}
          {tool.result && <>
            <div style={sty.toolDetailLabel}>result</div>
            <pre style={sty.toolDetailPre}>{result}</pre>
          </>}
        </div>
      )}
    </div>
  );
}
function tryFormatJson(s: string | undefined): string {
  if (!s) return "";
  try { return JSON.stringify(JSON.parse(s), null, 2); } catch { return s; }
}

// ─── FileChangeCard ───────────────────────────────────────────────────────────
const FILE_MUTATION_TOOLS = new Set(["write_file", "edit_file", "multi_edit", "apply_patch"]);

interface FileChange {
  path: string; action: "created" | "modified" | "patched";
  changes: Array<{ old: string; new: string }>; patch?: string; lang?: string;
}
function detectLang(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  const map: Record<string, string> = {
    rs: "rust", ts: "typescript", tsx: "tsx", js: "javascript", jsx: "jsx",
    py: "python", md: "markdown", toml: "toml", json: "json", yaml: "yaml",
    yml: "yaml", sh: "bash", html: "html", css: "css", go: "go",
  };
  return ext ? map[ext] : undefined;
}
function parseFileToolArgs(tool: ToolInvocation): FileChange | null {
  if (!tool.args) return null;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let args: any;
  try { args = JSON.parse(tool.args); } catch { return null; }
  switch (tool.name) {
    case "write_file":
      if (!args?.path) return null;
      return { path: args.path, action: "created", changes: [{ old: "", new: String(args.content ?? "") }], lang: detectLang(args.path) };
    case "edit_file":
      if (!args?.path) return null;
      return { path: args.path, action: "modified", changes: [{ old: String(args.old_string ?? ""), new: String(args.new_string ?? "") }], lang: detectLang(args.path) };
    case "multi_edit":
      if (!args?.path || !Array.isArray(args.edits)) return null;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      return { path: args.path, action: "modified", changes: args.edits.map((e: any) => ({ old: String(e.old_string ?? ""), new: String(e.new_string ?? "") })), lang: detectLang(args.path) };
    case "apply_patch": {
      if (typeof args.patch !== "string") return null;
      const m = /^\+\+\+ (?:b\/)?(.+)$/m.exec(args.patch);
      return { path: m ? m[1] : "(patch)", action: "patched", changes: [], patch: args.patch, lang: m ? detectLang(m[1]) : undefined };
    }
    default: return null;
  }
}

function UnifiedDiff({ oldText, newText }: { oldText: string; newText: string }) {
  const row = (line: string, kind: "del" | "add") => ({
    background: kind === "del" ? C.delBg : C.addBg,
    color: kind === "del" ? "#f87171" : "#4ade80",
    fontSize: 12.5, padding: "1px 14px", lineHeight: 1.6, fontFamily: "mono", whiteSpace: "pre" as const,
  });
  return (
    <div style={{ borderRadius: 7, overflow: "hidden", border: `1px solid ${C.border}`, margin: "6px 0", background: C.code }}>
      {oldText.split("\n").map((line, i) => <div key={`o${i}`} style={row(line, "del")}>{`- ${line || " "}`}</div>)}
      {oldText && newText && <div style={{ height: 1, background: C.border, opacity: 0.4 }}/>}
      {newText.split("\n").map((line, i) => <div key={`n${i}`} style={row(line, "add")}>{`+ ${line || " "}`}</div>)}
    </div>
  );
}

function FileChangeCard({ tool }: { tool: ToolInvocation }) {
  const [open, setOpen] = useState(false);
  const info = useMemo(() => parseFileToolArgs(tool), [tool]);
  if (!info) return <ToolChip tool={tool}/>;
  const actionColor = info.action === "created" ? "#4ade80" : info.action === "patched" ? "#60a5fa" : "#fcd34d";
  return (
    <div style={{ display: "flex", flexDirection: "column" as const }}>
      <button style={{ ...sty.toolChip, borderColor: `${actionColor}30` }} onClick={() => setOpen(o => !o)}>
        <span style={{ width: 6, height: 6, borderRadius: "50%", background: actionColor, flexShrink: 0 }}/>
        <span style={{ fontFamily: "mono", fontSize: 12, color: C.text, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" as const }}>{info.path}</span>
        <span style={{ fontSize: 10, color: actionColor, textTransform: "uppercase" as const }}>{info.action}</span>
        <IcChevron open={open}/>
      </button>
      {open && (
        <div style={{ marginTop: 4, padding: "8px 12px", background: "#0d0d14", border: `1px solid ${C.border}`, borderRadius: 8 }}>
          {info.patch
            ? <DiffBlock code={info.patch}/>
            : info.changes.map((c, i) => (
              <div key={i}>
                {info.changes.length > 1 && <div style={{ fontSize: 10, color: C.muted, fontWeight: 700, margin: "4px 0" }}>Hunk {i+1}</div>}
                {info.action === "created" ? <CodeBlock lang={info.lang} codeStr={c.new}/> : <UnifiedDiff oldText={c.old} newText={c.new}/>}
              </div>
            ))
          }
        </div>
      )}
    </div>
  );
}

function PendingToolChip({ name }: { name?: string }) {
  return (
    <div style={{ ...sty.toolChip, animation: "shimmer 1.4s ease-in-out infinite",
      background: "linear-gradient(90deg,#1a2238 25%,#243050 50%,#1a2238 75%)",
      backgroundSize: "200% 100%", cursor: "default", border: "1px solid #1e40af60" }}>
      <span style={{ width: 6, height: 6, borderRadius: "50%", background: "#60a5fa",
        animation: "pulse 1.2s ease-in-out infinite" }}/>
      <span style={{ fontFamily: "mono", opacity: 0.8 }}>{name ?? "running..."}</span>
    </div>
  );
}

// ─── Message bubbles ──────────────────────────────────────────────────────────
function UserBubble({ msg, onEdit, onDelete, onReply, onBranch }: {
  msg: Message; onEdit?: () => void; onDelete?: () => void;
  onReply?: () => void; onBranch?: () => void;
}) {
  return (
    <div style={sty.userRow}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
          <span style={{ fontSize: 12, fontWeight: 600, color: C.text }}>You</span>
          <span style={{ fontSize: 10, color: C.muted }} title={formatTimeAbsolute(msg.ts)}>{formatTimeAgo(msg.ts)}</span>
          <div style={sty.actionBar}>
            <button style={sty.actionBtn} onClick={() => navigator.clipboard.writeText(msg.text)} title="Copy">📋</button>
            {onEdit   && <button style={sty.actionBtn} onClick={onEdit}   title="Edit">✎</button>}
            {onReply  && <button style={sty.actionBtn} onClick={onReply}  title="Reply / quote">↩</button>}
            {onBranch && <button style={sty.actionBtn} onClick={onBranch} title="Branch from here">⎇</button>}
            {onDelete && <button style={sty.actionBtn} onClick={onDelete} title="Delete">🗑</button>}
          </div>
        </div>
        {msg.attachments && msg.attachments.length > 0 && (
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 6 }}>
            {msg.attachments.map(a => (
              <AttachmentPreview key={a.id} att={a}/>
            ))}
          </div>
        )}
        <div style={{ fontSize: 14, lineHeight: 1.75, color: C.text, whiteSpace: "pre-wrap" }}>{msg.text}</div>
      </div>
    </div>
  );
}

/** Compact preview chip for image / text attachments (used in user bubbles). */
function AttachmentPreview({ att }: { att: AttachmentDraft }) {
  if (att.kind === "image") {
    const src = att.preview ?? `data:${att.mime_type};base64,${att.data}`;
    return (
      <img src={src} alt={att.alt_text ?? "image attachment"}
        style={{ maxHeight: 80, maxWidth: 160, borderRadius: 6, border: `1px solid ${C.border}` }}/>
    );
  }
  return (
    <div style={sty.fileChip}>
      📄 <span style={{ fontSize: 11.5 }}>{att.filename ?? "file.txt"}</span>
    </div>
  );
}

function AssistantBubble({ msg, loading, isLast, onRegenerate, onCancelWorkflow, onResolveApproval, onReact, onBranch, onDelete }:
  { msg: Message; loading?: boolean; isLast?: boolean; onRegenerate?: () => void;
    onCancelWorkflow?: (traceId: string) => void;
    onResolveApproval?: (callId: string, approved: boolean) => void;
    onReact?: (r: "up" | "down") => void;
    onBranch?: () => void;
    onDelete?: () => void;
  }) {
  const [hovered, setHovered] = useState(false);
  const pendingApprovals = useMemo(
    () => (msg.toolCalls ?? []).filter(t => t.approval?.status === "pending"),
    [msg.toolCalls]
  );
  const events: AgentEvent[] = useMemo(() => {
    if (msg.events && msg.events.length > 0) return msg.events;
    const out: AgentEvent[] = [];
    if (msg.thinking) out.push({ kind: "thought", text: msg.thinking });
    for (const t of msg.toolCalls ?? []) out.push({ kind: "tool", tool: t });
    return out;
  }, [msg.events, msg.thinking, msg.toolCalls]);
  const hasActivity = events.length > 0;
  const showPending = loading && !msg.text && hasActivity;
  return (
    <div style={sty.assistantRow}
      onMouseEnter={() => setHovered(true)} onMouseLeave={() => setHovered(false)}>
      <div style={{ ...sty.avatar, ...(msg.streaming ? sty.avatarPulse : {}) }}><IcOwl/></div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
          <span style={{ fontSize: 12, fontWeight: 600, color: C.accent }}>Knight Owl</span>
          {!msg.streaming && (
            <>
              <span style={{ fontSize: 10, color: C.muted }} title={formatTimeAbsolute(msg.ts)}>{formatTimeAgo(msg.ts)}</span>
              {(msg.inputTokens != null || msg.outputTokens != null) && (
                <span style={{ fontSize: 10, color: C.muted }}>
                  {formatK((msg.inputTokens ?? 0) + (msg.outputTokens ?? 0))} tok
                </span>
              )}
              <div style={{ ...sty.actionBar, opacity: hovered ? 1 : 0.55 }}>
                {msg.text && (
                  <button style={sty.actionBtn} onClick={() => navigator.clipboard.writeText(msg.text)} title="Copy response">📋</button>
                )}
                {isLast && onRegenerate && (
                  <button style={sty.actionBtn} onClick={onRegenerate} title="Regenerate">↻</button>
                )}
                {onBranch && (
                  <button style={sty.actionBtn} onClick={onBranch} title="Branch from here">⎇</button>
                )}
                {onReact && (
                  <>
                    <button
                      style={{ ...sty.actionBtn, ...(msg.reaction === "up"   ? { color: "#4ade80" } : {}) }}
                      onClick={() => onReact("up")}   title="Helpful">👍</button>
                    <button
                      style={{ ...sty.actionBtn, ...(msg.reaction === "down" ? { color: "#f87171" } : {}) }}
                      onClick={() => onReact("down")} title="Not helpful">👎</button>
                  </>
                )}
                {onDelete && (
                  <button style={sty.actionBtn} onClick={onDelete} title="Delete">🗑</button>
                )}
              </div>
            </>
          )}
        </div>
        {(hasActivity || pendingApprovals.length > 0) && (
          <ActivityPanel
            events={events}
            streaming={msg.streaming}
            durationMs={msg.thinkingMs}
            tokenCount={msg.thinkingTokens}
            pendingApprovals={pendingApprovals}
            onResolveApproval={onResolveApproval}
          />
        )}
        {msg.streaming && msg.workflowTraceId && onCancelWorkflow && (
          <button style={sty.cancelBtn} onClick={() => onCancelWorkflow(msg.workflowTraceId!)}>
            Cancel workflow
          </button>
        )}
        {showPending && <PendingToolChip/>}
        {loading && !msg.text && !hasActivity
          ? <TypingDots/>
          : loading && !msg.text && hasActivity ? null
          : msg.text ? <MarkdownMessage text={msg.text} streaming={msg.streaming}/> : null
        }
        {msg.error && (
          <div style={sty.inlineError}>
            <span style={{ flex: 1, fontSize: 12.5 }}>{msg.error}</span>
            {onRegenerate && <button style={sty.retryBtn} onClick={onRegenerate}>Retry</button>}
          </div>
        )}
      </div>
    </div>
  );
}

function EmptyChat({ onPick }: { onPick: (text: string) => void }) {
  const examples = [
    "Explain the architecture of this codebase",
    "Find every TODO comment in the workspace",
    "Add tests for the multi_edit tool",
    "Why does the agent's tool loop stop early?",
  ];
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", height: "100%", gap: 16, padding: "0 48px" }}>
      <div style={{ width: 48, height: 48, borderRadius: "50%", background: C.accentBg, border: `1px solid ${C.accent}40`,
        display: "flex", alignItems: "center", justifyContent: "center", color: C.accent }}><IcOwl/></div>
      <h2 style={{ margin: 0, fontSize: 20, fontWeight: 600, color: C.text }}>How can I help you?</h2>
      <p style={{ margin: 0, fontSize: 13, color: C.muted, textAlign: "center", maxWidth: 380 }}>
        Pick a starter or ask anything about your workspace.
      </p>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10, width: "100%", maxWidth: 520, marginTop: 8 }}>
        {examples.map((ex, i) => (
          <button key={i} style={sty.exampleCard} onClick={() => onPick(ex)}>{ex}</button>
        ))}
      </div>
    </div>
  );
}

// ─── Icons ────────────────────────────────────────────────────────────────────
const IcPlus    = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>;
const IcSend    = () => <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z"/></svg>;
const IcStop    = () => <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>;
const IcTools   = () => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>;
const IcGear    = () => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>;
const IcAgents  = () => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z"/><path d="M2 22v-2a4 4 0 0 1 4-4h6a4 4 0 0 1 4 4v2"/><path d="M16 3a4 4 0 0 1 0 8"/><path d="M22 22v-2a4 4 0 0 0-3-3.87"/></svg>;
const IcGraph   = () => <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><circle cx="6" cy="6" r="3"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="18" r="3"/><path d="M8.5 7.5L15.5 16.5"/><path d="M15.5 7.5L8.5 16.5"/></svg>;
const IcTrash   = () => <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>;
const IcEdit    = () => <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>;
const IcArrowDown = () => <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><line x1="12" y1="5" x2="12" y2="19"/><polyline points="19 12 12 19 5 12"/></svg>;
const IcChevron = ({ open }: { open: boolean }) => (
  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round"
    style={{ transform: open ? "rotate(90deg)" : "rotate(0deg)", transition: "transform 0.15s", flexShrink: 0 }}>
    <polyline points="9 18 15 12 9 6"/>
  </svg>
);
const IcOwl = () => (
  <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
    <circle cx="9" cy="10" r="2"/><circle cx="15" cy="10" r="2"/>
    <path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2z"/>
    <path d="M8.5 16.5s1 1.5 3.5 1.5 3.5-1.5 3.5-1.5"/>
    <path d="M9 8c0 0-1-2-3-2"/><path d="M15 8c0 0 1-2 3-2"/>
  </svg>
);
const IcClose = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
    <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
  </svg>
);
const IcSidebar = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
    <rect x="3" y="3" width="18" height="18" rx="2"/><line x1="9" y1="3" x2="9" y2="21"/>
  </svg>
);

// ─── Slash commands ───────────────────────────────────────────────────────────
interface SlashItem {
  cmd: string; icon: string; label: string; desc: string;
  kind: "builtin" | "text" | "workflow";
  workflow?: string;
}
const BUILTIN_SLASH_COMMANDS: SlashItem[] = [
  { cmd: "clear", icon: "x", label: "Clear conversation", desc: "Remove all messages", kind: "builtin" },
  { cmd: "reset", icon: "r", label: "Reset history", desc: "Clear messages and backend memory", kind: "builtin" },
  { cmd: "index", icon: "i", label: "Index workspace", desc: "Parse code into SurrealDB", kind: "builtin" },
  { cmd: "help",  icon: "?", label: "Show help", desc: "List commands", kind: "builtin" },
];

function SlashMenu({ items, filter, selectedIdx, onSelect }: {
  items: SlashItem[]; filter: string; selectedIdx: number; onSelect: (cmd: string) => void;
}) {
  const f = filter.toLowerCase();
  const matches = items.filter(c => !filter || c.cmd.toLowerCase().startsWith(f) || c.label.toLowerCase().includes(f));
  if (matches.length === 0) return null;
  return (
    <div style={sty.slashMenu}>
      {matches.map((c, i) => (
        <button key={c.cmd} style={{ ...sty.slashItem, ...(i === selectedIdx ? sty.slashItemActive : {}) }}
          onMouseDown={e => { e.preventDefault(); onSelect(c.cmd); }}>
          <div style={{ flex: 1 }}>
            <span style={{ color: C.text, fontSize: 13, fontWeight: 500 }}>/{c.cmd}</span>
            <span style={{ color: C.muted, fontSize: 11.5, marginLeft: 8 }}>{c.label}</span>
            {c.kind !== "builtin" && (
              <span style={{ fontSize: 9.5, marginLeft: 6, color: C.accent,
                background: C.accentBg, padding: "1px 5px", borderRadius: 3, textTransform: "uppercase" as const }}>
                {c.kind}
              </span>
            )}
          </div>
        </button>
      ))}
    </div>
  );
}

// ─── KB Types ─────────────────────────────────────────────────────────────────
interface KbStats { file_count: number; node_count: number; edge_count: number; entity_count: number; insight_count: number; }
interface FileWithStats { path: string; lang: string; node_count: number; }
interface CodeNode { id: string; file_path: string; name: string; kind: string; start_line: number; end_line: number; preview: string; visibility: string; qualifiers: string; description: string; }
interface CodeEdge { from: string; to: string; kind: string; }
interface GraphData { nodes: CodeNode[]; edges: CodeEdge[]; }
interface KbInsight { id: string; kind: string; scope: string; summary: string; evidence: string[]; }

const NODE_COLORS: Record<string, string> = {
  function: "#22c55e", method: "#22c55e", struct: "#4a9eff", enum: "#a855f7",
  trait: "#f59e0b", impl: "#64748b", module: "#ec4899", constant: "#94a3b8",
};
const EDGE_COLORS: Record<string, string> = {
  DEFINES: "#555", CONTAINS: "#4a9eff", CALLS: "#22c55e",
  REFERENCES: "#f59e0b", IMPLEMENTS: "#a855f7",
};

// ─── Side Panel: Knowledge Base ───────────────────────────────────────────────
function KnowledgeBasePanel() {
  const [stats, setStats] = useState<KbStats|null>(null);
  const [files, setFiles] = useState<FileWithStats[]>([]);
  const [selectedFile, setSelectedFile] = useState<string|null>(null);
  const [graph, setGraph] = useState<GraphData|null>(null);
  const [selectedNode, setSelectedNode] = useState<CodeNode|null>(null);
  const [neighbors, setNeighbors] = useState<CodeNode[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<CodeNode[]>([]);
  const [tab, setTab] = useState<"files"|"search"|"insights">("files");
  const [insights, setInsights] = useState<KbInsight[]>([]);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    invoke<KbStats>("kb_stats").then(setStats).catch(() => {});
    invoke<FileWithStats[]>("kb_files").then(setFiles).catch(() => {});
    invoke<KbInsight[]>("kb_insights", { scope: null }).then(setInsights).catch(() => {});
  }, []);

  const selectFile = useCallback(async (path: string) => {
    setSelectedFile(path); setSelectedNode(null);
    try { const g = await invoke<GraphData>("kb_file_graph", { path }); setGraph(g); }
    catch { setGraph(null); }
  }, []);

  const selectNode = useCallback(async (node: CodeNode) => {
    setSelectedNode(node);
    try { const n = await invoke<CodeNode[]>("kb_node_neighbors", { id: node.id, depth: 1 }); setNeighbors(n); }
    catch { setNeighbors([]); }
  }, []);

  const doSearch = useCallback(async () => {
    if (!searchQuery.trim()) return;
    try {
      const r = await invoke<CodeNode[]>("kb_search", { query: searchQuery, mode: "hybrid", limit: 20 });
      setSearchResults(r); setTab("search");
    } catch {}
  }, [searchQuery]);

  // Canvas graph
  useEffect(() => {
    if (!graph || !canvasRef.current) return;
    const canvas = canvasRef.current;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const W = canvas.width = canvas.offsetWidth * 2;
    const H = canvas.height = canvas.offsetHeight * 2;
    ctx.scale(2,2);
    const w = W/2, h = H/2;
    ctx.fillStyle = "#0d1117"; ctx.fillRect(0,0,w,h);
    if (graph.nodes.length === 0) {
      ctx.fillStyle = "#8b949e"; ctx.font = "13px system-ui";
      ctx.textAlign = "center"; ctx.fillText("No nodes", w/2, h/2);
      return;
    }
    const positions = new Map<string, {x:number,y:number}>();
    const cx = w/2, cy = h/2, radius = Math.min(w,h) * 0.35;
    graph.nodes.forEach((n, i) => {
      const angle = (2*Math.PI*i)/graph.nodes.length - Math.PI/2;
      positions.set(n.id, { x: cx + radius*Math.cos(angle), y: cy + radius*Math.sin(angle) });
    });
    graph.edges.forEach(e => {
      const from = positions.get(e.from), to = positions.get(e.to);
      if (!from || !to) return;
      ctx.strokeStyle = EDGE_COLORS[e.kind] || "#333"; ctx.lineWidth = 1; ctx.globalAlpha = 0.5;
      ctx.beginPath(); ctx.moveTo(from.x, from.y); ctx.lineTo(to.x, to.y); ctx.stroke();
    });
    ctx.globalAlpha = 1;
    graph.nodes.forEach(n => {
      const p = positions.get(n.id); if (!p) return;
      const r = n.id === selectedNode?.id ? 7 : 4;
      ctx.fillStyle = NODE_COLORS[n.kind] || "#8b949e";
      ctx.beginPath(); ctx.arc(p.x, p.y, r, 0, 2*Math.PI); ctx.fill();
      if (n.id === selectedNode?.id) { ctx.strokeStyle = "#fff"; ctx.lineWidth = 2; ctx.beginPath(); ctx.arc(p.x, p.y, r+2, 0, 2*Math.PI); ctx.stroke(); }
      ctx.fillStyle = "#c9d1d9"; ctx.font = "9px system-ui"; ctx.textAlign = "center";
      ctx.fillText(n.name, p.x, p.y + r + 11);
    });
    const handleClick = (e: MouseEvent) => {
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left, my = e.clientY - rect.top;
      for (const n of graph.nodes) {
        const p = positions.get(n.id);
        if (!p) continue;
        if (Math.hypot(mx - p.x, my - p.y) < 10) { selectNode(n); return; }
      }
    };
    canvas.addEventListener("click", handleClick);
    return () => canvas.removeEventListener("click", handleClick);
  }, [graph, selectedNode, selectNode]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", fontSize: 12 }}>
      {/* Stats */}
      <div style={{ display: "flex", gap: 12, padding: "10px 14px", borderBottom: `1px solid ${C.border}`, color: C.muted, flexWrap: "wrap" }}>
        <span><b style={{ color: C.text }}>{stats?.file_count ?? "-"}</b> files</span>
        <span><b style={{ color: C.text }}>{stats?.node_count ?? "-"}</b> nodes</span>
        <span><b style={{ color: C.text }}>{stats?.insight_count ?? "-"}</b> insights</span>
      </div>
      {/* Search */}
      <div style={{ display: "flex", padding: "8px 14px", gap: 6, borderBottom: `1px solid ${C.border}` }}>
        <input style={sty.panelInput} placeholder="Search code..." value={searchQuery}
          onChange={e => setSearchQuery(e.target.value)} onKeyDown={e => e.key === "Enter" && doSearch()}/>
        <button style={sty.panelBtn} onClick={doSearch}>Go</button>
      </div>
      {/* Tabs */}
      <div style={{ display: "flex", borderBottom: `1px solid ${C.border}` }}>
        {(["files","search","insights"] as const).map(t => (
          <button key={t} style={{ ...sty.panelTab, ...(tab === t ? { color: C.text, borderBottomColor: C.accent } : {}) }}
            onClick={() => setTab(t)}>{t === "files" ? "Files" : t === "search" ? "Results" : "Insights"}</button>
        ))}
      </div>
      {/* Content */}
      <div style={{ flex: 1, overflowY: "auto" }}>
        {tab === "files" && (
          <div>
            {files.map(f => (
              <div key={f.path} style={{ ...sty.panelListItem, ...(selectedFile === f.path ? { background: "#1f2937", color: C.text } : {}) }}
                onClick={() => selectFile(f.path)}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{f.path}</span>
                <span style={{ color: "#4a9eff", fontSize: 10, flexShrink: 0 }}>{f.node_count}</span>
              </div>
            ))}
            {files.length === 0 && <p style={{ color: C.muted, padding: "12px 14px" }}>No indexed files. Run /index first.</p>}
          </div>
        )}
        {tab === "search" && (
          <div>
            {searchResults.length === 0 && <p style={{ color: C.muted, padding: "12px 14px" }}>No results</p>}
            {searchResults.map(n => (
              <div key={n.id} style={sty.panelListItem} onClick={() => selectNode(n)}>
                <span style={{ color: NODE_COLORS[n.kind] || C.muted, fontWeight: 600 }}>{n.kind}</span>
                <span style={{ color: C.text, flex: 1 }}>{n.name}</span>
                <span style={{ color: C.muted, fontSize: 10 }}>{n.file_path.split("/").pop()}:{n.start_line}</span>
              </div>
            ))}
          </div>
        )}
        {tab === "insights" && (
          <div>
            <div style={{ padding: "8px 14px", borderBottom: `1px solid ${C.border}` }}>
              <button style={sty.panelBtn} onClick={async () => {
                try {
                  await invoke<string[]>("kb_distill");
                  const fresh = await invoke<KbInsight[]>("kb_insights", { scope: null });
                  setInsights(fresh);
                  const s = await invoke<KbStats>("kb_stats"); setStats(s);
                } catch (e) { alert(`Distillation failed: ${e}`); }
              }}>Distill</button>
            </div>
            {insights.length === 0 && <p style={{ color: C.muted, padding: "12px 14px" }}>No insights yet.</p>}
            {insights.map(ins => (
              <div key={ins.id} style={{ padding: "8px 14px", borderBottom: `1px solid ${C.border}` }}>
                <span style={{ fontSize: 10, fontWeight: 700, textTransform: "uppercase" as const,
                  color: ins.kind === "AntiPattern" ? "#f85149" : ins.kind === "Pattern" ? "#22c55e" : "#f59e0b" }}>
                  {ins.kind}
                </span>
                <div style={{ color: C.text, marginTop: 2 }}>{ins.summary}</div>
                <div style={{ color: C.muted, fontSize: 10, marginTop: 2 }}>scope: {ins.scope}</div>
              </div>
            ))}
          </div>
        )}
      </div>
      {/* Graph canvas + detail */}
      {selectedFile && (
        <div style={{ borderTop: `1px solid ${C.border}`, height: 200, position: "relative" }}>
          <canvas ref={canvasRef} style={{ width: "100%", height: "100%" }}/>
        </div>
      )}
      {selectedNode && (
        <div style={{ borderTop: `1px solid ${C.border}`, padding: "8px 14px", maxHeight: 200, overflowY: "auto" }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: C.text }}>{selectedNode.name}</div>
          <div style={{ fontSize: 10, color: C.muted }}>{selectedNode.kind} - {selectedNode.file_path}:{selectedNode.start_line}</div>
          {selectedNode.description && <div style={{ fontSize: 11, color: C.text, marginTop: 4 }}>{selectedNode.description}</div>}
          {neighbors.length > 0 && (
            <div style={{ marginTop: 6 }}>
              <span style={{ fontSize: 10, color: C.muted }}>Connected ({neighbors.length}):</span>
              {neighbors.map(n => (
                <div key={n.id} style={{ fontSize: 10, color: "#4a9eff", cursor: "pointer", padding: "1px 0" }}
                  onClick={() => selectNode(n)}>{n.kind}: {n.name}</div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Side Panel: Tools & MCP ──────────────────────────────────────────────────
function ToolsPanel() {
  const [tab, setTab] = useState<"mcp"|"native">("mcp");
  const [nativeTools, setNativeTools] = useState<ToolInfo[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServerConfig[]>([]);
  const [modal, setModal] = useState<{ mode: "add" } | { mode: "edit"; server: McpServerConfig } | null>(null);

  const reload = useCallback(async () => {
    try {
      const [tools, servers] = await Promise.all([
        invoke<ToolInfo[]>("list_native_tools"),
        invoke<McpServerConfig[]>("list_mcp_servers"),
      ]);
      setNativeTools(tools); setMcpServers(servers);
    } catch (e) { console.error(e); }
  }, []);
  useEffect(() => { reload(); }, [reload]);

  const handleSave = async (cfg: McpServerConfig) => {
    try {
      await invoke(modal?.mode === "edit" ? "update_mcp_server" : "add_mcp_server", { config: cfg });
      setModal(null); reload();
    } catch (e) { alert(String(e)); }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", fontSize: 12 }}>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 14px", gap: 6, borderBottom: `1px solid ${C.border}` }}>
        {(["mcp","native"] as const).map(t => (
          <button key={t} style={{ ...sty.panelTab, ...(tab === t ? { color: C.text, borderBottomColor: C.accent } : {}) }}
            onClick={() => setTab(t)}>{t === "mcp" ? "MCP Servers" : "Native Tools"}</button>
        ))}
        {tab === "mcp" && <button style={{ ...sty.panelBtn, marginLeft: "auto" }} onClick={() => setModal({ mode: "add" })}>+ Add</button>}
      </div>
      <div style={{ flex: 1, overflowY: "auto", padding: "8px 14px" }}>
        {tab === "mcp" && mcpServers.map(srv => (
          <div key={srv.id} style={{ ...sty.card, marginBottom: 6, opacity: srv.enabled ? 1 : 0.5 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <div style={{ width: 6, height: 6, borderRadius: "50%", background: srv.enabled ? "#4ade80" : C.muted }}/>
              <span style={{ fontWeight: 600, color: C.text, flex: 1 }}>{srv.name}</span>
              <span style={{ fontSize: 10, color: C.muted }}>{srv.transport}</span>
              <button style={sty.miniBtn} onClick={() => setModal({ mode: "edit", server: srv })}>Edit</button>
              <button style={sty.miniBtn} onClick={async () => {
                await invoke("update_mcp_server", { config: { ...srv, enabled: !srv.enabled } }); reload();
              }}>{srv.enabled ? "Off" : "On"}</button>
              <button style={{ ...sty.miniBtn, color: "#f87171" }} onClick={async () => {
                await invoke("remove_mcp_server", { id: srv.id }); reload();
              }}>Del</button>
            </div>
          </div>
        ))}
        {tab === "mcp" && mcpServers.length === 0 && <p style={{ color: C.muted }}>No MCP servers configured.</p>}
        {tab === "native" && nativeTools.map(t => (
          <div key={t.name} style={{ ...sty.card, marginBottom: 6 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span style={{ fontWeight: 600, color: C.text }}>{t.name}</span>
              <span style={{ fontSize: 10, color: C.muted, background: "#ffffff08", padding: "1px 5px", borderRadius: 3 }}>
                {t.source.type === "mcp" ? (t.source as {type:"mcp";server_name:string}).server_name : "native"}
              </span>
            </div>
            <p style={{ margin: "4px 0 0", color: C.muted, lineHeight: 1.5 }}>{t.description}</p>
          </div>
        ))}
        {tab === "native" && nativeTools.length === 0 && <p style={{ color: C.muted }}>No native tools.</p>}
      </div>
      {modal && (
        <div style={sty.overlay} onClick={e => { if (e.target === e.currentTarget) setModal(null); }}>
          <div style={sty.modalBox}>
            <h3 style={{ margin: "0 0 16px", fontSize: 14, fontWeight: 600, color: C.text }}>
              {modal.mode === "add" ? "Add MCP Server" : `Edit: ${modal.server.name}`}
            </h3>
            <McpForm initial={modal.mode === "edit" ? modal.server : {}} onSave={handleSave} onCancel={() => setModal(null)}/>
          </div>
        </div>
      )}
    </div>
  );
}

function McpForm({ initial, onSave, onCancel }: { initial: Partial<McpServerConfig>; onSave: (c: McpServerConfig) => void; onCancel: () => void }) {
  const [name, setName] = useState(initial.name ?? "");
  const [transport, setTransport] = useState<McpTransportKind>(initial.transport ?? "stdio");
  const [command, setCommand] = useState(initial.command ?? "");
  const [argsStr, setArgsStr] = useState((initial.args ?? []).join(" "));
  const [envStr, setEnvStr] = useState((initial.env ?? []).join("\n"));
  const [url, setUrl] = useState(initial.url ?? "");
  const [enabled, setEnabled] = useState(initial.enabled ?? true);
  const inp: React.CSSProperties = { background: C.inputBg, border: `1px solid ${C.border}`, borderRadius: 8,
    color: C.text, fontSize: 13, padding: "8px 11px", width: "100%", outline: "none", fontFamily: "inherit" };
  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    onSave({ id: initial.id ?? newUuid(), name: name.trim(), transport,
      command: command.trim() || undefined, args: argsStr.trim() ? argsStr.trim().split(/\s+/) : [],
      env: envStr.trim() ? envStr.trim().split("\n").filter(Boolean) : [],
      url: url.trim() || undefined, enabled });
  };
  return (
    <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <label style={sty.formLabel}>Name<input style={inp} value={name} onChange={e=>setName(e.target.value)} required autoFocus/></label>
      <label style={sty.formLabel}>Transport
        <select style={inp} value={transport} onChange={e=>setTransport(e.target.value as McpTransportKind)}>
          <option value="stdio">stdio</option><option value="web_socket">WebSocket</option><option value="http">HTTP</option>
        </select>
      </label>
      {transport === "stdio" && <>
        <label style={sty.formLabel}>Command<input style={inp} value={command} onChange={e=>setCommand(e.target.value)}/></label>
        <label style={sty.formLabel}>Args<input style={inp} value={argsStr} onChange={e=>setArgsStr(e.target.value)}/></label>
        <label style={sty.formLabel}>Env<textarea style={{ ...inp, resize: "none", height: 60 }} value={envStr} onChange={e=>setEnvStr(e.target.value)}/></label>
      </>}
      {(transport === "web_socket" || transport === "http") && <label style={sty.formLabel}>URL<input style={inp} value={url} onChange={e=>setUrl(e.target.value)}/></label>}
      <label style={{ display: "flex", alignItems: "center", gap: 8, color: C.muted, cursor: "pointer" }}>
        <input type="checkbox" checked={enabled} onChange={e=>setEnabled(e.target.checked)} style={{ accentColor: C.accent }}/>Enabled
      </label>
      <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", paddingTop: 4, borderTop: `1px solid ${C.border}` }}>
        <button type="button" style={sty.btnGhost} onClick={onCancel}>Cancel</button>
        <button type="submit" style={sty.btnPrimary}>Save</button>
      </div>
    </form>
  );
}

// ─── Side Panel: Agents ───────────────────────────────────────────────────────
function AgentsPanel() {
  const [tab, setTab] = useState<"agents"|"skills"|"workflows"|"commands">("agents");
  const [agents, setAgents] = useState<AgentListItem[]>([]);
  const [skills, setSkills] = useState<SkillListItem[]>([]);
  const [workflows, setWorkflows] = useState<WorkflowListItem[]>([]);
  const [commands, setCommands] = useState<CommandListItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<string|null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const [a, s, w, c] = await Promise.all([
        invoke<AgentListItem[]>("list_orchestra_agents"),
        invoke<SkillListItem[]>("list_orchestra_skills"),
        invoke<WorkflowListItem[]>("list_orchestra_workflows"),
        invoke<CommandListItem[]>("list_orchestra_commands"),
      ]);
      setAgents(a); setSkills(s); setWorkflows(w); setCommands(c);
    } catch (e) { console.error(e); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { reload(); }, [reload]);
  useEffect(() => { setSelected(null); }, [tab]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", fontSize: 12 }}>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 14px", gap: 4, borderBottom: `1px solid ${C.border}`, flexWrap: "wrap" }}>
        {(["agents","skills","workflows","commands"] as const).map(t => (
          <button key={t} style={{ ...sty.panelTab, ...(tab === t ? { color: C.text, borderBottomColor: C.accent } : {}) }}
            onClick={() => setTab(t)}>{t.charAt(0).toUpperCase()+t.slice(1)}</button>
        ))}
        <button style={{ ...sty.miniBtn, marginLeft: "auto" }} onClick={reload} disabled={loading}>
          {loading ? "..." : "Refresh"}
        </button>
      </div>
      <div style={{ flex: 1, overflowY: "auto", padding: "8px 14px" }}>
        {tab === "agents" && agents.map(a => (
          <div key={a.id} style={{ ...sty.card, marginBottom: 6, ...(selected === a.id ? { borderColor: C.accent+"80" } : {}) }}
            onClick={() => setSelected(selected === a.id ? null : a.id)}>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span style={{ fontWeight: 600, color: C.text }}>{a.name}</span>
              <code style={{ fontSize: 10, color: C.muted }}>{a.id}</code>
            </div>
            <p style={{ margin: "2px 0 0", color: C.muted }}>{a.description}</p>
            {selected === a.id && (
              <div style={{ marginTop: 8, paddingTop: 8, borderTop: `1px solid ${C.border}`, fontSize: 11, color: C.muted }}>
                <div>max steps: {a.max_steps} | skills: {a.skills.length}</div>
                <div>tools: {a.allowed_tools.length === 0 ? "(all)" : a.allowed_tools.join(", ")}</div>
              </div>
            )}
          </div>
        ))}
        {tab === "agents" && agents.length === 0 && <p style={{ color: C.muted }}>No agents defined.</p>}
        {tab === "skills" && skills.map(sk => (
          <div key={sk.id} style={{ ...sty.card, marginBottom: 6 }}>
            <span style={{ fontWeight: 600, color: C.text }}>{sk.name}</span>
            <p style={{ margin: "2px 0 0", color: C.muted }}>{sk.description}</p>
          </div>
        ))}
        {tab === "skills" && skills.length === 0 && <p style={{ color: C.muted }}>No skills defined.</p>}
        {tab === "workflows" && workflows.map(w => (
          <div key={w.id} style={{ ...sty.card, marginBottom: 6 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span style={{ fontWeight: 600, color: C.text }}>{w.name}</span>
              <span style={{ fontSize: 10, color: C.muted }}>{w.steps.length} steps</span>
            </div>
            <p style={{ margin: "2px 0 0", color: C.muted }}>{w.description}</p>
          </div>
        ))}
        {tab === "workflows" && workflows.length === 0 && <p style={{ color: C.muted }}>No workflows.</p>}
        {tab === "commands" && commands.map(c => (
          <div key={c.id} style={{ ...sty.card, marginBottom: 6 }}>
            <code style={{ color: C.accent, fontWeight: 600 }}>/{c.name}</code>
            <span style={{ fontSize: 10, color: C.muted, marginLeft: 6 }}>{c.kind}</span>
            <p style={{ margin: "2px 0 0", color: C.muted }}>{c.description}</p>
          </div>
        ))}
        {tab === "commands" && commands.length === 0 && <p style={{ color: C.muted }}>No commands.</p>}
        <p style={{ marginTop: 16, color: C.muted, fontSize: 11, lineHeight: 1.6 }}>
          Edit specs under <code style={sty.inlineCode}>.knight-owl/</code> - changes hot-reload.
        </p>
      </div>
    </div>
  );
}

// ─── Side Panel: Settings ─────────────────────────────────────────────────────
const SETTING_FIELDS: Array<{ key: keyof AppSettings; label: string; placeholder: string; secret?: boolean; envVar: string; group: string }> = [
  { key: "gemini_api_key", group: "Gemini", label: "API key", envVar: "GEMINI_API_KEY", placeholder: "AIza...", secret: true },
  { key: "gemini_model", group: "Gemini", label: "Model", envVar: "OWL_GEMINI_MODEL", placeholder: "gemini-2.5-flash" },
  { key: "gemini_embedding_model", group: "Gemini", label: "Embedding", envVar: "OWL_GEMINI_EMBEDDING_MODEL", placeholder: "gemini-embedding-002" },
  { key: "anthropic_api_key", group: "Anthropic", label: "API key", envVar: "ANTHROPIC_API_KEY", placeholder: "sk-ant-...", secret: true },
  { key: "surreal_endpoint", group: "SurrealDB", label: "Endpoint", envVar: "OWL_VAULT_ENDPOINT", placeholder: "ws://localhost:8000" },
  { key: "surreal_namespace", group: "SurrealDB", label: "Namespace", envVar: "OWL_VAULT_NAMESPACE", placeholder: "knight_owl" },
  { key: "surreal_database", group: "SurrealDB", label: "Database", envVar: "OWL_VAULT_DATABASE", placeholder: "vault" },
  { key: "surreal_username", group: "SurrealDB", label: "Username", envVar: "OWL_VAULT_USER", placeholder: "root" },
  { key: "surreal_password", group: "SurrealDB", label: "Password", envVar: "OWL_VAULT_PASS", placeholder: "root", secret: true },
];

function SettingsPanel() {
  const [view, setView] = useState<SettingsView|null>(null);
  const [draft, setDraft] = useState<AppSettings>({});
  const [reveal, setReveal] = useState<Record<string, boolean>>({});
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const reload = useCallback(async () => {
    try { const v = await invoke<SettingsView>("get_settings"); setView(v); setDraft(v.settings ?? {}); }
    catch (e) { console.error(e); }
  }, []);
  useEffect(() => { reload(); }, [reload]);

  const handleSave = async () => {
    setSaving(true); setSaved(false);
    try {
      const cleaned: AppSettings = {};
      for (const k of Object.keys(draft) as (keyof AppSettings)[]) {
        const v = draft[k]; if (v && v.trim()) cleaned[k] = v.trim();
      }
      await invoke("save_settings", { settings: cleaned });
      setSaved(true); setTimeout(() => setSaved(false), 2400); reload();
    } catch (e) { alert("Save failed: " + String(e)); }
    finally { setSaving(false); }
  };

  if (!view) return <div style={{ padding: 14, color: C.muted }}>Loading...</div>;
  const groups = ["Gemini", "Anthropic", "SurrealDB"];

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", fontSize: 12 }}>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 14px", gap: 8, borderBottom: `1px solid ${C.border}` }}>
        <span style={{ flex: 1, fontWeight: 600, color: C.text }}>Settings</span>
        {saved && <span style={{ color: "#4ade80", fontSize: 11 }}>Saved - restart to apply</span>}
        <button style={sty.panelBtn} onClick={handleSave} disabled={saving}>{saving ? "..." : "Save"}</button>
      </div>
      <div style={{ flex: 1, overflowY: "auto", padding: "8px 14px" }}>
        <p style={{ color: C.muted, marginBottom: 12, lineHeight: 1.5 }}>
          Stored at <code style={sty.inlineCode}>{view.path}</code>. Env vars win.
        </p>
        {groups.map(group => (
          <div key={group} style={{ marginBottom: 16 }}>
            <h4 style={{ margin: "0 0 8px", fontSize: 12, fontWeight: 700, color: C.text, borderBottom: `1px solid ${C.border}`, paddingBottom: 4 }}>{group}</h4>
            {SETTING_FIELDS.filter(f => f.group === group).map(f => {
              const fromEnv = view.env_set[f.key];
              const value = (draft[f.key] ?? "") as string;
              return (
                <label key={f.key} style={{ display: "flex", flexDirection: "column", gap: 3, marginBottom: 8, color: C.muted }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                    <span>{f.label}</span>
                    {fromEnv && <span style={{ fontSize: 9, color: "#4ade80", background: "#0d2a0d", padding: "1px 5px", borderRadius: 3 }}>ENV</span>}
                  </div>
                  <div style={{ display: "flex", gap: 4 }}>
                    <input style={{ ...sty.panelInput, opacity: fromEnv ? 0.5 : 1 }}
                      type={f.secret && !reveal[f.key] ? "password" : "text"}
                      placeholder={fromEnv ? "(from env)" : f.placeholder}
                      value={value} disabled={fromEnv}
                      onChange={e => setDraft(d => ({ ...d, [f.key]: e.target.value }))}/>
                    {f.secret && !fromEnv && (
                      <button type="button" style={sty.miniBtn}
                        onClick={() => setReveal(r => ({ ...r, [f.key]: !r[f.key] }))}>{reveal[f.key] ? "Hide" : "Show"}</button>
                    )}
                  </div>
                </label>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

// ─── Approval card ────────────────────────────────────────────────────────────
/**
 * Inline confirm UI for tools gated by `RequireApproval` policy on the
 * backend.  Renders inside a tool-call card whenever an `agent_event` of
 * `tool_awaiting_approval` arrives.  Click → invoke `resolve_tool_approval`.
 *
 * Fail-safe: if the user closes the app without responding, the backend
 * gate's parked sender drops and resolves to `Rejected` automatically (see
 * `apps/owl-desktop/src-tauri/src/approval.rs`).
 */
function ApprovalCard({ tool, onResolve }: {
  tool: ToolInvocation;
  onResolve: (approved: boolean) => void;
}) {
  const argsPretty = (() => {
    if (!tool.args) return "";
    try { return JSON.stringify(JSON.parse(tool.args), null, 2); }
    catch { return tool.args; }
  })();
  return (
    <div style={sty.approvalCard}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 6 }}>
        <span style={sty.approvalBadge}>⚠ approval required</span>
        <span style={{ fontWeight: 600, fontSize: 13, color: C.text }}>{tool.name}</span>
      </div>
      {tool.approval?.reason && (
        <div style={{ fontSize: 12, color: C.muted, marginBottom: 6 }}>{tool.approval.reason}</div>
      )}
      {argsPretty && (
        <pre style={sty.approvalArgs}>{argsPretty}</pre>
      )}
      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <button style={sty.approveBtn} onClick={() => onResolve(true)}>✓ Approve & run</button>
        <button style={sty.rejectBtn}  onClick={() => onResolve(false)}>✗ Reject</button>
      </div>
    </div>
  );
}

// ─── Sprite debugger — visual frame picker ────────────────────────────────────
/**
 * Fullscreen overlay that renders all 54 frames of the Owlbert spritesheet
 * in a numbered 6×9 grid.  Use to identify which row corresponds to which
 * action (idle, happy, eating, sleeping, …) so we can fix the
 * `OWLBERT.animations` mapping.
 *
 * Open from the pet's action menu ("🔧 Debug frames") or from DevTools:
 *   window.dispatchEvent(new Event("owl-pet-debug-frames"))
 */
function SpriteDebugger({ onClose }: { onClose: () => void }) {
  const [hoverFrame, setHoverFrame] = useState<number | null>(null);
  const [preview, setPreview]       = useState<number[]>([]);

  const togglePreview = (i: number) => {
    setPreview(prev => prev.includes(i) ? prev.filter(x => x !== i) : [...prev, i]);
  };

  return (
    <div style={dbgSty.overlay} onClick={onClose}>
      <div style={dbgSty.modal} onClick={e => e.stopPropagation()}>
        <div style={dbgSty.header}>
          <strong style={{ fontSize: 14 }}>Owlbert spritesheet — pick frames per action</strong>
          <div style={{ flex: 1 }}/>
          <button style={dbgSty.copyBtn}
            onClick={() => navigator.clipboard.writeText(`[${preview.join(", ")}]`)}>
            Copy [{preview.join(",")}]
          </button>
          <button style={dbgSty.closeBtn} onClick={onClose}>×</button>
        </div>
        <div style={{ display: "flex", gap: 16, padding: 12 }}>
          {/* Numbered grid of every frame */}
          <div style={{
            display: "grid",
            gridTemplateColumns: `repeat(${OWLBERT.cols}, 96px)`,
            gap: 4,
          }}>
            {Array.from({ length: OWLBERT.cols * OWLBERT.rows }, (_, i) => {
              const col = i % OWLBERT.cols;
              const row = Math.floor(i / OWLBERT.cols);
              const scale = 96 / OWLBERT.frameW;
              const picked = preview.includes(i);
              return (
                <div key={i}
                  onClick={() => togglePreview(i)}
                  onMouseEnter={() => setHoverFrame(i)}
                  onMouseLeave={() => setHoverFrame(null)}
                  style={{
                    width: 96, height: Math.round(OWLBERT.frameH * scale),
                    border: `2px solid ${picked ? "#cc785c" : (hoverFrame === i ? "#888" : "#2a2a38")}`,
                    borderRadius: 4, cursor: "pointer", position: "relative",
                    backgroundImage:    `url(${OWLBERT.url})`,
                    backgroundSize:     `${OWLBERT.frameW * OWLBERT.cols * scale}px ${OWLBERT.frameH * OWLBERT.rows * scale}px`,
                    backgroundPosition: `-${col * OWLBERT.frameW * scale}px -${row * OWLBERT.frameH * scale}px`,
                    backgroundRepeat:   "no-repeat",
                  }}>
                  <span style={{
                    position: "absolute", top: 2, left: 4,
                    fontSize: 10, fontWeight: 700, color: "#fff",
                    textShadow: "0 1px 2px #000, 0 0 4px #000",
                  }}>{i}</span>
                  {picked && (
                    <span style={{
                      position: "absolute", bottom: 2, right: 4,
                      fontSize: 10, fontWeight: 700, color: "#cc785c",
                      textShadow: "0 1px 2px #000",
                    }}>#{preview.indexOf(i) + 1}</span>
                  )}
                </div>
              );
            })}
          </div>
          {/* Sidebar: live preview of selected sequence */}
          <div style={{ width: 260, display: "flex", flexDirection: "column", gap: 12 }}>
            <div style={dbgSty.helpText}>
              <div><b>Hover</b> → see frame index</div>
              <div><b>Click</b> → add to sequence</div>
              <div><b>Click again</b> → remove</div>
              <div style={{ marginTop: 8 }}>Sequence is the order frames will play in.</div>
            </div>
            {preview.length > 0 && (
              <div style={dbgSty.previewBox}>
                <div style={{ fontSize: 11, color: "#999", marginBottom: 6 }}>
                  Preview ({preview.length} frames @ 8 fps):
                </div>
                <PreviewPlayer frames={preview} fps={8}/>
                <div style={{ fontSize: 10, color: "#777", marginTop: 8,
                  fontFamily: "'JetBrains Mono','Fira Code','Menlo',monospace" }}>
                  frames: [{preview.join(", ")}]
                </div>
                <button style={{ ...dbgSty.copyBtn, marginTop: 8, width: "100%" }}
                  onClick={() => setPreview([])}>Clear selection</button>
              </div>
            )}
            {preview.length === 0 && (
              <div style={{ ...dbgSty.helpText, opacity: 0.6 }}>
                Click frames in the grid to build a sequence and preview it here.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function PreviewPlayer({ frames, fps }: { frames: number[]; fps: number }) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick(t => (t + 1) % frames.length), 1000 / fps);
    return () => clearInterval(id);
  }, [frames, fps]);
  const f = frames[tick] ?? 0;
  const col = f % OWLBERT.cols;
  const row = Math.floor(f / OWLBERT.cols);
  const scale = 200 / OWLBERT.frameW;
  return (
    <div style={{
      width: 200, height: OWLBERT.frameH * scale,
      backgroundImage:    `url(${OWLBERT.url})`,
      backgroundSize:     `${OWLBERT.frameW * OWLBERT.cols * scale}px ${OWLBERT.frameH * OWLBERT.rows * scale}px`,
      backgroundPosition: `-${col * OWLBERT.frameW * scale}px -${row * OWLBERT.frameH * scale}px`,
      backgroundRepeat:   "no-repeat",
      border: `1px solid ${C.border}`, borderRadius: 6, margin: "0 auto",
    }}/>
  );
}

const dbgSty: Record<string, React.CSSProperties> = {
  overlay: {
    position: "fixed", inset: 0, background: "#000000d0", zIndex: 2000,
    display: "flex", alignItems: "center", justifyContent: "center",
  },
  modal: {
    background: "#1a1a28", borderRadius: 12, border: `1px solid ${C.border}`,
    maxWidth: "95vw", maxHeight: "92vh", overflow: "auto", boxShadow: "0 20px 60px rgba(0,0,0,0.7)",
  },
  header: {
    display: "flex", alignItems: "center", gap: 8,
    padding: "10px 14px", borderBottom: `1px solid ${C.border}`,
  },
  closeBtn: {
    width: 28, height: 28, borderRadius: 6, border: "none", background: "#2a2a38",
    color: "#fff", fontSize: 16, cursor: "pointer",
  },
  copyBtn: {
    padding: "4px 10px", borderRadius: 6, border: `1px solid ${C.border}`,
    background: "#2a2a38", color: "#fff", fontSize: 11, cursor: "pointer",
    fontFamily: "'JetBrains Mono','Fira Code','Menlo',monospace",
  },
  helpText: {
    background: "#0a0a14", border: `1px solid ${C.border}`, borderRadius: 8,
    padding: 10, fontSize: 11.5, color: "#aaa", lineHeight: 1.6,
  },
  previewBox: {
    background: "#0a0a14", border: `1px solid ${C.accent}40`, borderRadius: 8, padding: 10,
  },
};

// ─── Owl chibi pet widget ─────────────────────────────────────────────────────
/**
 * Floating draggable pet that reacts to agent events.
 *
 * Backend ↔ frontend wiring:
 * - On mount → `invoke('get_pet')` for initial snapshot.
 * - Listens `pet_event` Tauri channel for live updates (every tool call,
 *   feed, decay tick).  Payload: `{ pet: PetData, leveled_up: boolean }`.
 * - Click pet → action menu (Feed / Play / Pet / Rename).
 * - `last_say` bubble auto-hides after 4 s.
 * - Persists position to localStorage so it doesn't jump every reload.
 */
function OwlPetWidget() {
  const [pet, setPet] = useState<PetData | null>(null);
  const [menu, setMenu] = useState(false);
  const [bubble, setBubble] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [renameDraft, setRenameDraft] = useState("");
  const [celebrate, setCelebrate] = useState(false);
  /** Sprite debugger modal — open via menu or global event. */
  const [debugOpen, setDebugOpen] = useState(false);
  useEffect(() => {
    const onOpen = () => setDebugOpen(true);
    window.addEventListener("owl-pet-debug-frames", onOpen);
    return () => window.removeEventListener("owl-pet-debug-frames", onOpen);
  }, []);
  /** Transient one-shot animation triggered by the user clicking
   *  Feed/Play/Pet — overrides the ambient mood anim for the duration of
   *  the sequence, then auto-clears back to `null` so the mood resumes. */
  const [actionAnim, setActionAnim] = useState<AnimKey | null>(null);
  /** Minimized: hides stats + name tag, shows only the owl bubble. */
  const [minimized, setMinimized] = useState(() => localStorage.getItem("owl-pet-min") === "1");
  /** Hidden completely (toggle via topbar / right-click). */
  const [hidden, setHidden] = useState(() => localStorage.getItem("owl-pet-hidden") === "1");
  /** Auto-fade when the chat composer textarea is focused. */
  const [composerFocused, setComposerFocused] = useState(false);

  useEffect(() => { localStorage.setItem("owl-pet-min",    minimized ? "1" : "0"); }, [minimized]);
  useEffect(() => { localStorage.setItem("owl-pet-hidden", hidden    ? "1" : "0"); }, [hidden]);

  // Topbar 🦉 button fires `owl-pet-show` to bring the pet back without
  // requiring an app reload.
  useEffect(() => {
    const onShow = () => setHidden(false);
    window.addEventListener("owl-pet-show", onShow);
    return () => window.removeEventListener("owl-pet-show", onShow);
  }, []);

  // Watch focusin / focusout on the document for the chat textarea so the
  // pet politely fades out while the user is typing instead of overlapping
  // the composer.
  useEffect(() => {
    const isComposer = (el: Element | null) =>
      el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;
    const onFocusIn  = (e: FocusEvent) => { if (isComposer(e.target as Element)) setComposerFocused(true);  };
    const onFocusOut = (e: FocusEvent) => { if (isComposer(e.target as Element)) setComposerFocused(false); };
    document.addEventListener("focusin",  onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    return () => {
      document.removeEventListener("focusin",  onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
    };
  }, []);

  // Position: load + persist.  Default = top-right of viewport, clear of
  // both the side panel zone AND the composer at the bottom.
  const [pos, setPos] = useState(() => {
    try {
      const raw = localStorage.getItem("owl-pet-pos");
      if (raw) {
        const p = JSON.parse(raw) as { x: number; y: number };
        // Validate against current viewport in case window was resized.
        if (p.x >= 0 && p.x <= window.innerWidth - 60 && p.y >= 0 && p.y <= window.innerHeight - 60) {
          return p;
        }
      }
    } catch {}
    return { x: window.innerWidth - 100, y: 70 };
  });
  useEffect(() => { localStorage.setItem("owl-pet-pos", JSON.stringify(pos)); }, [pos]);

  // Re-clamp on window resize so the pet never gets stranded off-screen.
  useEffect(() => {
    const resize = () => setPos(p => ({
      x: Math.max(8, Math.min(window.innerWidth  - 80, p.x)),
      y: Math.max(8, Math.min(window.innerHeight - 80, p.y)),
    }));
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);

  // Dragging — snap to nearest horizontal edge on release so the pet always
  // rests against the left or right side and never floats in the middle
  // of the chat where it would block content.
  const dragRef = useRef<{ dx: number; dy: number; moved: boolean } | null>(null);
  const onMouseDown = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("[data-pet-action]")) return; // don't drag from buttons
    dragRef.current = { dx: e.clientX - pos.x, dy: e.clientY - pos.y, moved: false };
    document.body.style.userSelect = "none";
  };
  useEffect(() => {
    const move = (e: globalThis.MouseEvent) => {
      if (!dragRef.current) return;
      dragRef.current.moved = true;
      setPos({
        x: Math.max(8, Math.min(window.innerWidth - 80, e.clientX - dragRef.current.dx)),
        y: Math.max(8, Math.min(window.innerHeight - 80, e.clientY - dragRef.current.dy)),
      });
    };
    const up = () => {
      if (dragRef.current?.moved) {
        // Snap to nearest horizontal edge.
        setPos(p => {
          const leftDist  = p.x;
          const rightDist = window.innerWidth - p.x - 64;
          return { ...p, x: leftDist < rightDist ? 8 : window.innerWidth - 80 };
        });
      }
      dragRef.current = null;
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup",   up);
    return () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); };
  }, []);

  // Right-click → context menu with hide/minimize toggles.
  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    if (window.confirm("Hide pet? (toggle from the top bar to bring back)")) {
      setHidden(true);
    }
  };

  // Initial fetch + subscribe.
  useEffect(() => {
    invoke<PetData>("get_pet").then(setPet).catch(() => {});
    const sub = listen<{ pet: PetData; leveled_up: boolean }>("pet_event", e => {
      setPet(e.payload.pet);
      if (e.payload.pet.last_say) {
        setBubble(e.payload.pet.last_say);
        setTimeout(() => setBubble(null), 4000);
      }
      if (e.payload.leveled_up) {
        setCelebrate(true);
        setTimeout(() => setCelebrate(false), 2500);
      }
    });
    return () => { sub.then(un => un()); };
  }, []);

  const mood: PetMood = useMemo(() => {
    if (!pet) return "content";
    if (pet.energy    <= 5)  return "sleeping";
    if (pet.energy    <= 20) return "tired";
    if (pet.hunger    <= 30) return "hungry";
    if (pet.happiness <= 30) return "sad";
    if (pet.happiness >= 75) return "happy";
    return "content";
  }, [pet]);

  if (!pet || hidden) return null;

  const form = pet.level <= 2 ? "egg"
    : pet.level <= 4 ? "chick"
    : pet.level <= 9 ? "owlet"
    : pet.level <= 19 ? "adult"
    : "sage";

  /** Map a Tauri command to the one-shot animation it should trigger. */
  const CMD_ANIM: Record<"feed_pet" | "play_pet" | "pet_pet", AnimKey> = {
    feed_pet: "eating",
    play_pet: "playing",
    pet_pet:  "petted",
  };

  const action = async (cmd: "feed_pet" | "play_pet" | "pet_pet") => {
    setMenu(false);
    const animKey = CMD_ANIM[cmd];
    // Trigger the one-shot animation in parallel with the backend call.
    // Animation duration ≈ frames / fps × 1000 ms.  Reset to ambient after.
    const a = OWLBERT.animations[animKey];
    const ms = Math.round((a.frames.length / a.fps) * 1000) + 80;
    setActionAnim(animKey);
    setTimeout(() => setActionAnim(null), ms);
    try { await invoke(cmd); } catch (e) { console.warn(cmd, e); }
  };

  const submitRename = async () => {
    const n = renameDraft.trim();
    if (!n) { setRenaming(false); return; }
    try {
      const updated = await invoke<PetData>("rename_pet", { name: n });
      setPet(updated);
    } catch (e) { console.warn("rename", e); }
    setRenaming(false);
  };

  // Open menu to the LEFT when pet is snapped to the right edge.
  const menuOnLeft = pos.x > window.innerWidth / 2;
  const containerOpacity = composerFocused && !menu ? 0.35 : 1;

  return (
    <div data-pet-root
      style={{
        position: "fixed", left: pos.x, top: pos.y, zIndex: 1000, fontFamily: FONT,
        opacity: containerOpacity, transition: "opacity 0.2s ease",
      }}
      onMouseDown={onMouseDown}
      onContextMenu={onContextMenu}>
      {/* Close button on hover — discoverable way to hide. */}
      <button data-pet-action data-pet-close style={petSty.closeBtn}
        onClick={() => setHidden(true)} title="Hide pet">×</button>
      {/* Speech bubble */}
      {bubble && (
        <div style={petSty.bubble}>{bubble}</div>
      )}
      {/* Level-up confetti */}
      {celebrate && (
        <div style={petSty.celebrate}>✨ LEVEL UP ✨</div>
      )}
      {/* Pet body — single click toggles menu, dbl-click toggles minimize. */}
      <button data-pet-action
        style={{ ...petSty.body, ...(minimized ? petSty.bodyMin : {}) }}
        onClick={() => setMenu(m => !m)}
        onDoubleClick={e => { e.stopPropagation(); setMinimized(m => !m); setMenu(false); }}
        title={`${pet.name} · Lv ${pet.level} · ${form}\nClick: menu · Dbl-click: ${minimized ? "expand" : "minimize"} · Right-click: hide`}>
        <OwlChibi form={form} mood={mood} small={minimized} action={actionAnim}/>
      </button>
      {/* Name tag (hidden when minimized) */}
      {!minimized && (
        <div style={petSty.nameTag}>
          {renaming
            ? <input autoFocus style={petSty.renameInput} value={renameDraft}
                onChange={e => setRenameDraft(e.target.value)}
                onBlur={submitRename}
                onKeyDown={e => { if (e.key === "Enter") submitRename(); if (e.key === "Escape") setRenaming(false); }}/>
            : <span onDoubleClick={() => { setRenameDraft(pet.name); setRenaming(true); }}
                title="Double-click to rename" style={{ cursor: "text" }}>
                {pet.name} <span style={{ opacity: 0.6, fontSize: 9 }}>Lv{pet.level}</span>
              </span>
          }
        </div>
      )}
      {/* Stats (hidden when minimized) */}
      {!minimized && (
        <div style={petSty.stats}>
          <Stat label="🍎" value={pet.hunger}    color="#fb923c"/>
          <Stat label="😊" value={pet.happiness} color="#facc15"/>
          <Stat label="⚡" value={pet.energy}    color="#60a5fa"/>
        </div>
      )}
      {/* Action menu — pops left/right depending on where pet is docked. */}
      {menu && (
        <div data-pet-action style={{
          ...petSty.menu,
          ...(menuOnLeft ? { right: 72, left: "auto" } : { left: 72, right: "auto" }),
        }}>
          <button style={petSty.menuItem} onClick={() => action("feed_pet")}>🍎 Feed</button>
          <button style={petSty.menuItem} onClick={() => action("play_pet")}>🎾 Play</button>
          <button style={petSty.menuItem} onClick={() => action("pet_pet")}>🤚 Pet</button>
          <button style={petSty.menuItem}
            onClick={() => { setMenu(false); setRenameDraft(pet.name); setRenaming(true); }}>
            ✎ Rename
          </button>
          <button style={petSty.menuItem} onClick={() => { setMenu(false); setMinimized(m => !m); }}>
            {minimized ? "◳ Expand" : "▭ Minimize"}
          </button>
          <button style={petSty.menuItem} onClick={() => { setMenu(false); setDebugOpen(true); }}>
            🔧 Debug frames
          </button>
          <button style={petSty.menuItem} onClick={() => { setMenu(false); setHidden(true); }}>
            👋 Hide
          </button>
        </div>
      )}
      {debugOpen && <SpriteDebugger onClose={() => setDebugOpen(false)}/>}
    </div>
  );
}

/** Tiny show-pet toggle button placed by the top bar (see AppState wiring). */
function PetShowButton({ onShow }: { onShow: () => void }) {
  return (
    <button onClick={onShow} title="Show owl pet" style={petSty.showBtn}>🦉</button>
  );
}

function Stat({ label, value, color }: { label: string; value: number; color: string }) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
      <span style={{ fontSize: 10 }}>{label}</span>
      <div style={petSty.statBar}>
        <div style={{ ...petSty.statFill, width: `${value}%`, background: color }}/>
      </div>
    </div>
  );
}

// ─── Owlbert spritesheet (codex-pets format) ─────────────────────────────────
// Asset: /public/pets/owlbert/spritesheet.webp
// Dimensions: 1536×1872 px → 6 cols × 9 rows × 256×208 px/frame (54 frames).
//
// Each row is treated as one animation sequence (6 frames).  Mood/action
// maps to a sequence + fps.  If a particular sequence looks "off" relative
// to the asset, just edit the `frames` array — no renderer changes needed.

interface SpriteRect {
  x: number; y: number; w: number; h: number;
  /** Optional debug label. */
  note?: string;
}

const SHEET = {
  url:    "/pets/owlbert/spritesheet.webp",
  sheetW: 1536,
  sheetH: 1872,
} as const;

/**
 * Per-mood static crop rectangle.  Coordinates extracted via connected-
 * component blob detection on the spritesheet (`/tmp/owl-frames/`):
 * - idle/content : alert front-facing owl, top-left of sheet
 * - happy/excited: wing-up "hi" pose, middle band
 * - hungry       : calm sitting owl, middle band
 * - sad/tired    : drooping-eyed owl, lower middle band
 * - sleeping     : eyes-closed peaceful owl
 *
 * Animation comes from CSS body transforms (breathe / wiggle / bounce /
 * sleep) — the sprite itself is static so we never teleport between
 * unrelated frames.  One-shot action sprites swap in briefly when the
 * user clicks Feed/Play/Pet.
 */
const MOOD_SPRITE: Record<PetMood, SpriteRect> = {
  content:  { x:  22, y:    5, w: 147, h: 198, note: "alert front-facing" },
  happy:    { x: 389, y:  631, w: 182, h: 194, note: "wing-up greeting"   },
  excited:  { x: 389, y:  631, w: 182, h: 194, note: "wing-up greeting"   },
  hungry:   { x: 389, y:  837, w: 182, h: 198, note: "calm sitting"       },
  tired:    { x: 393, y: 1045, w: 173, h: 198, note: "droopy eyes"        },
  sad:      { x: 393, y: 1045, w: 173, h: 198, note: "droopy eyes"        },
  sleeping: { x: 581, y: 1049, w: 182, h: 190, note: "eyes closed"        },
};

const ACTION_SPRITE: Record<"eating" | "playing" | "petted", SpriteRect> = {
  eating:   { x: 389, y:  837, w: 182, h: 198, note: "looking down to eat" },
  playing:  { x: 389, y:  631, w: 182, h: 194, note: "wing-up play"        },
  petted:   { x: 581, y: 1049, w: 182, h: 190, note: "eyes-closed bliss"   },
};

type AnimKey = keyof typeof ACTION_SPRITE;

// Legacy grid model kept for the in-app sprite debugger overlay only —
// runtime renderer no longer reads `OWLBERT.animations`, it uses
// MOOD_SPRITE / ACTION_SPRITE crop rectangles defined above.
type Anim = { frames: number[]; fps: number; loop?: boolean };
const OWLBERT = {
  url:    SHEET.url,
  frameW: 256,
  frameH: 208,
  cols:   6,
  rows:   9,
  /**
   * Animation library.  Each row of the spritesheet is a 6-frame cycle;
   * mood/action picks which row plays.  The row guesses are based on the
   * codex-pets convention (row 0 = idle, sad/sleep rows near the middle,
   * happy / wing-flap rows near the bottom).  Tune `frames` if any mood
   * doesn't visually match.
   */
  /**
   * Frame map for Owlbert.  The codex-pets spritesheet is *sparse*: many
   * grid cells are empty padding (purple bg) rather than real frames, so
   * naively cycling through `[0..6]` of a row teleports the pet into a
   * blank.  We therefore cherry-pick a SMALL number of visually similar,
   * confirmed-non-blank frames per mood and pair them with rich CSS body
   * transforms (`breathe` / `bob` / `wiggle`) for the bulk of the motion.
   *
   * Frames confirmed by inspecting the sheet (1536×1872, 6 cols × 9 rows):
   * - 0-2     : front-facing idle (alert eyes open)
   * - 6, 8    : same family, slight head tilt
   * - 30, 34  : sad / drooping eyes
   * - 32, 33  : sleeping / closed eyes
   * - 38      : winking (one eye closed)
   * - 36, 42  : variants of standing
   *
   * Use the in-app "🔧 Debug frames" tool to retune if any look wrong.
   */
  animations: {
    // Ambient — subtle 2-frame ping-pong on confirmed-adjacent frames,
    // slow fps so the change reads as "breathing" not "twitching".
    idle:     { frames: [0, 1],     fps: 1.2, loop: true } as Anim,
    content:  { frames: [0, 1],     fps: 1.2, loop: true } as Anim,
    happy:    { frames: [0, 2, 1],  fps: 2.5, loop: true } as Anim,
    excited:  { frames: [0, 2, 1],  fps: 4,   loop: true } as Anim,
    hungry:   { frames: [6, 8],     fps: 1.2, loop: true } as Anim,
    tired:    { frames: [30],       fps: 1,   loop: true } as Anim,
    sad:      { frames: [30, 34],   fps: 0.8, loop: true } as Anim,
    sleeping: { frames: [32, 33],   fps: 0.7, loop: true } as Anim,
    // One-shots — short and visually self-contained, NOT looping.
    // Each ends back at the ambient idle frame to land cleanly.
    eating:   { frames: [6, 0, 6, 0, 1],   fps: 6, loop: false } as Anim,
    playing:  { frames: [2, 0, 2, 1, 0],   fps: 6, loop: false } as Anim,
    petted:   { frames: [38, 0, 1],        fps: 4, loop: false } as Anim,
  },
} as const;


/**
 * Sprite-based pet avatar using **per-mood crop rectangles**.
 *
 * The sheet's irregular layout means uniform grid slicing produced
 * jitter (frames landing in gutters).  Instead each mood/action maps to
 * one precise (x, y, w, h) box of a single owl sprite.  All motion comes
 * from CSS body transforms; the sprite itself is static within a mood.
 *
 * - Ambient sprite = `MOOD_SPRITE[mood]`
 * - One-shot (Feed/Play/Pet click) briefly swaps to `ACTION_SPRITE[action]`
 * - Falls back to emoji if the spritesheet 404s OR the pet is still
 *   pre-hatched (egg / chick — no hatch sprites in the sheet).
 */
function OwlChibi({ form, mood, small, action }: {
  form:   string;
  mood:   PetMood;
  small?: boolean;
  action?: AnimKey | null;
}) {
  const [assetOk, setAssetOk] = useState(true);

  // Asset probe — disable sprite path if file is missing.
  useEffect(() => {
    const img = new Image();
    img.onerror = () => setAssetOk(false);
    img.src = SHEET.url;
  }, []);

  // Pre-evolution forms keep the emoji fallback (no hatch sprites).
  if (!assetOk || form === "egg" || form === "chick") {
    const glyph = form === "egg"   ? "🥚"
                : form === "chick" ? "🐣"
                : mood === "sleeping" ? "💤"
                : mood === "sad"   ? "😿"
                : "🦉";
    return (
      <span style={{ fontSize: small ? 24 : 44, display: "inline-block", lineHeight: 1,
        animation: mood === "happy" ? "owl-pet-bounce 1s ease-in-out infinite"
                : mood === "sleeping" ? "owl-pet-sleep 2s ease-in-out infinite"
                : "owl-pet-idle 3s ease-in-out infinite" }}>
        {glyph}
      </span>
    );
  }

  // Pick which sprite rectangle to show: explicit action overrides mood.
  const rect: SpriteRect = action ? ACTION_SPRITE[action] : MOOD_SPRITE[mood];

  // Preserve the sprite's aspect ratio so a 147×198 owl doesn't get
  // squished into a 60×60 box.  Compute display H from W.
  const displayW = small ? 36 : 60;
  const displayH = Math.round(displayW * rect.h / rect.w);
  const scale = displayW / rect.w;
  const bgW   = SHEET.sheetW * scale;
  const bgH   = SHEET.sheetH * scale;

  // CSS body transform — drives all motion since the sprite itself is static.
  const bodyAnim = mood === "sleeping" ? "owl-pet-sleep 3.4s ease-in-out infinite"
                 : mood === "happy"    ? "owl-pet-wiggle 1.6s ease-in-out infinite"
                 : mood === "excited"  ? "owl-pet-bounce 0.55s ease-in-out infinite"
                 : mood === "sad"      ? "owl-pet-droop 3s ease-in-out infinite"
                 : mood === "tired"    ? "owl-pet-droop 4s ease-in-out infinite"
                 : mood === "hungry"   ? "owl-pet-breathe 2.4s ease-in-out infinite"
                 : "owl-pet-breathe 2.8s ease-in-out infinite";
  const tint = mood === "sad"   ? "grayscale(0.4)"
             : mood === "tired" ? "brightness(0.85)"
             : "none";

  return (
    <span style={{
      display: "inline-block",
      width:  displayW,
      height: displayH,
      backgroundImage:    `url(${SHEET.url})`,
      backgroundSize:     `${bgW}px ${bgH}px`,
      backgroundPosition: `-${rect.x * scale}px -${rect.y * scale}px`,
      backgroundRepeat:   "no-repeat",
      imageRendering:     "auto",
      animation: bodyAnim, filter: tint,
    }}/>
  );
}

const petSty: Record<string, React.CSSProperties> = {
  body: {
    width: 64, height: 64, borderRadius: "50%",
    background: "linear-gradient(135deg, #2a2030, #1a1428)",
    border: `2px solid ${C.accent}80`,
    boxShadow: "0 4px 20px rgba(0,0,0,0.5), inset 0 2px 4px rgba(255,255,255,0.05)",
    cursor: "grab",
    display: "flex", alignItems: "center", justifyContent: "center",
    padding: 0,
  },
  nameTag: {
    marginTop: 4, textAlign: "center", fontSize: 11, color: C.text,
    fontWeight: 600, textShadow: "0 1px 2px rgba(0,0,0,0.8)",
  },
  renameInput: {
    width: 80, fontSize: 11, padding: "2px 6px", borderRadius: 4,
    border: `1px solid ${C.accent}`, background: C.surface, color: C.text, outline: "none",
  },
  stats: {
    marginTop: 4, display: "flex", flexDirection: "column", gap: 2,
    background: "#1a1a28cc", padding: "4px 6px", borderRadius: 8,
    border: `1px solid ${C.border}`, backdropFilter: "blur(6px)",
  },
  statBar: {
    width: 60, height: 5, background: "#0a0a14", borderRadius: 99, overflow: "hidden",
  },
  statFill: { height: "100%", borderRadius: 99, transition: "width 0.4s ease" },
  bubble: {
    position: "absolute", bottom: 80, left: "50%", transform: "translateX(-50%)",
    background: "#fff", color: "#111", padding: "5px 10px", borderRadius: 12,
    fontSize: 12, fontWeight: 600, whiteSpace: "nowrap" as const,
    boxShadow: "0 4px 10px rgba(0,0,0,0.4)",
    animation: "owl-pet-bubble 0.3s ease-out",
  },
  celebrate: {
    position: "absolute", bottom: 105, left: "50%", transform: "translateX(-50%)",
    color: "#facc15", fontWeight: 800, fontSize: 13, whiteSpace: "nowrap" as const,
    textShadow: "0 0 8px #facc15, 0 2px 4px rgba(0,0,0,0.6)",
    animation: "owl-pet-celebrate 2.5s ease-out forwards",
  },
  menu: {
    position: "absolute", left: 72, top: 0, background: C.surface,
    border: `1px solid ${C.border}`, borderRadius: 10, padding: 4,
    display: "flex", flexDirection: "column", gap: 2, minWidth: 110,
    boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
  },
  menuItem: {
    textAlign: "left", padding: "6px 10px", background: "transparent",
    border: "none", color: C.text, fontSize: 12.5, borderRadius: 6, cursor: "pointer",
  },
  bodyMin: { width: 36, height: 36, borderWidth: 1.5 },
  closeBtn: {
    position: "absolute", top: -6, right: -6, width: 18, height: 18,
    borderRadius: "50%", background: "#2a2a38", border: `1px solid ${C.border}`,
    color: C.muted, fontSize: 12, lineHeight: "14px", padding: 0,
    cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center",
    opacity: 0, transition: "opacity 0.15s", zIndex: 1,
  },
  showBtn: {
    background: "transparent", border: `1px solid ${C.border}`, borderRadius: 6,
    width: 28, height: 28, color: C.muted, fontSize: 14, cursor: "pointer",
    display: "inline-flex", alignItems: "center", justifyContent: "center",
  },
};

// ─── Root App ─────────────────────────────────────────────────────────────────
export default function App() {
  const [convs, setConvs] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<string|null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string|null>(null);
  const [sidePanel, setSidePanel] = useState<SidePanel>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [workspace, setWorkspace] = useState<WorkspaceInfo|null>(null);
  const [recentWorkspaces, setRecentWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [needsRestart, setNeedsRestart] = useState(false);
  const [indexProgress, setIndexProgress] = useState<IndexProgress|null>(null);

  // Composer-staged attachments + global agent_event listener (separate from
  // the legacy stream_chunk path so both data streams coexist).
  const [attachments, setAttachments] = useState<AttachmentDraft[]>([]);

  const streamRef = useRef<{ unlisten: () => void } | null>(null);
  const thinkingStartRef = useRef<number>(0);

  // Slash menu state
  const [draft, setDraft] = useState("");
  const [slashIdx, setSlashIdx] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [atBottom, setAtBottom] = useState(true);

  const activeConv = convs.find(c => c.id === activeId) ?? null;

  const newConv = useCallback(() => {
    const id = newUuid();
    setConvs(p => [{ id, title: "New conversation", messages: [], ts: Date.now() }, ...p]);
    setActiveId(id); setError(null);
  }, []);

  const hydratedRef = useRef(false);

  // Load persisted conversations
  useEffect(() => {
    let cancelled = false;
    invoke<string>("list_conversations").then(json => {
      if (cancelled) return;
      try {
        const loaded = JSON.parse(json) as Conversation[];
        if (Array.isArray(loaded) && loaded.length > 0) {
          const sanitized = loaded.map(c => ({ ...c, messages: c.messages.map(m => ({ ...m, streaming: false })) }));
          setConvs(sanitized); setActiveId(sanitized[0].id);
        } else { newConv(); }
      } catch { newConv(); }
      hydratedRef.current = true;
    }).catch(() => { newConv(); hydratedRef.current = true; });
    return () => { cancelled = true; };
  }, [newConv]);

  // Auto-save
  useEffect(() => {
    if (!hydratedRef.current) return;
    const t = setTimeout(() => {
      const payload = convs.map(c => ({ ...c, messages: c.messages.map(m => ({ ...m, streaming: false })) }));
      invoke("save_conversations", { json: JSON.stringify(payload) }).catch(console.error);
    }, 500);
    return () => clearTimeout(t);
  }, [convs]);

  // Workspaces
  const reloadWorkspaces = useCallback(() => {
    invoke<WorkspaceList>("list_workspaces").then(list => {
      setWorkspace(list.active); setRecentWorkspaces(list.recent);
    }).catch(console.error);
  }, []);
  useEffect(() => { reloadWorkspaces(); }, [reloadWorkspaces]);

  // Orchestra agents + commands
  const [agents, setAgents] = useState<AgentListItem[]>([]);
  const [customCommands, setCustomCommands] = useState<CommandListItem[]>([]);
  const reloadAgents = useCallback(() => {
    invoke<AgentListItem[]>("list_orchestra_agents").then(setAgents).catch(console.error);
    invoke<CommandListItem[]>("list_orchestra_commands").then(setCustomCommands).catch(console.error);
  }, []);
  useEffect(() => { reloadAgents(); }, [reloadAgents]);
  useEffect(() => {
    const onFocus = () => reloadAgents();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [reloadAgents]);

  const handleSetAgent = useCallback((agentId: string | null) => {
    if (!activeId) return;
    setConvs(p => p.map(c => c.id === activeId ? { ...c, agent_id: agentId ?? undefined } : c));
  }, [activeId]);

  // Slash commands
  const slashMatch = /^\/(\w*)$/.exec(draft.trim());
  const slashFilter = slashMatch ? slashMatch[1] : null;
  const slashOpen = slashFilter !== null;

  useEffect(() => { if (slashOpen) reloadAgents(); }, [slashOpen, reloadAgents]);

  const allSlashItems: SlashItem[] = useMemo(() => {
    const custom: SlashItem[] = customCommands.map(c => ({
      cmd: c.name, icon: c.kind === "workflow" ? ">" : "t", label: c.description || c.name,
      desc: c.kind === "workflow" ? "Workflow" : c.kind === "tool" ? "Tool" : "Text",
      kind: c.kind === "workflow" ? "workflow" : c.kind === "tool" ? "builtin" : "text",
      workflow: c.kind === "workflow" ? (c.workflow_id ?? c.id) : undefined,
    }));
    return [...BUILTIN_SLASH_COMMANDS, ...custom];
  }, [customCommands]);

  const slashCmds = slashOpen
    ? allSlashItems.filter(c => !slashFilter || c.cmd.toLowerCase().startsWith(slashFilter.toLowerCase()))
    : [];

  useEffect(() => { setSlashIdx(0); }, [slashFilter]);

  // Index progress
  useEffect(() => {
    const p = listen<IndexProgress>("index_progress", e => setIndexProgress(e.payload));
    return () => { p.then(un => un()); };
  }, []);

  // ── agent_event channel (brain-emitted step-level + token-level stream) ───
  // Drives: token streaming, tool-call cards, inline approval prompts.
  // Streams to whichever conversation the most recent assistant message
  // belongs to — backend `commands::stream` binds the active_conv before
  // each turn so events can't cross-contaminate convs.
  useEffect(() => {
    const handler = listen<BackendAgentEvent>("agent_event", (event) => {
      const ev = event.payload;
      setConvs(prev => {
        // Find the conv with the most-recent streaming assistant message
        // (defensive: events arrive between our optimistic message creation
        // and the loop's first emission, so we may receive events for a
        // message that has already finished — they're simply dropped).
        const idx = prev.findIndex(c => c.messages.some(m => m.streaming && m.role === "assistant"));
        if (idx < 0) return prev;
        const conv = prev[idx];
        const msgIdx = conv.messages.findIndex(m => m.streaming && m.role === "assistant");
        if (msgIdx < 0) return prev;
        const msg = conv.messages[msgIdx];

        const updated: Message = (() => {
          switch (ev.type) {
            case "text_chunk":
              return { ...msg, text: msg.text + ev.content };
            case "tool_calling": {
                const tool: ToolInvocation = {
                  name: ev.call.name,
                  args: typeof ev.call.args === "string" ? ev.call.args : JSON.stringify(ev.call.args),
                };
                const toolCalls = [...(msg.toolCalls ?? []), tool];
                const events: AgentEvent[] = [...(msg.events ?? []), { kind: "tool", tool }];
                return { ...msg, toolCalls, events };
              }
            case "tool_called": {
                const calls = msg.toolCalls ?? [];
                // Match the most recent matching tool call by name without a result yet.
                const i = [...calls].reverse().findIndex(t => t.name === ev.result.name && t.result == null);
                if (i < 0) return msg;
                const realI = calls.length - 1 - i;
                const updatedTool: ToolInvocation = {
                  ...calls[realI],
                  result: typeof ev.result.output === "string" ? ev.result.output : JSON.stringify(ev.result.output),
                  error:  ev.result.success ? undefined : (ev.result.error ?? "tool failed"),
                };
                const toolCalls = calls.map((t, ix) => ix === realI ? updatedTool : t);
                const events: AgentEvent[] = [...(msg.events ?? []), { kind: "tool", tool: updatedTool }];
                return { ...msg, toolCalls, events };
              }
            case "tool_awaiting_approval": {
                const tool: ToolInvocation = {
                  name: ev.call.name,
                  args: typeof ev.call.args === "string" ? ev.call.args : JSON.stringify(ev.call.args),
                  approval: { call_id: ev.call_id, status: "pending", reason: ev.reason },
                };
                const toolCalls = [...(msg.toolCalls ?? []), tool];
                const events: AgentEvent[] = [...(msg.events ?? []), { kind: "tool", tool }];
                return { ...msg, toolCalls, events };
              }
            case "approval_resolved": {
                const calls = msg.toolCalls ?? [];
                const i = calls.findIndex(t => t.approval?.call_id === ev.call_id);
                if (i < 0) return msg;
                const updatedTool: ToolInvocation = {
                  ...calls[i],
                  approval: { ...calls[i].approval!, status: ev.approved ? "approved" : "rejected" },
                };
                return { ...msg, toolCalls: calls.map((t, ix) => ix === i ? updatedTool : t) };
              }
            case "error":
              return { ...msg, error: ev.message };
            case "done":
            case "state_changed":
              return msg; // already handled by stream_chunk's done branch / state UI
          }
        })();

        const messages = [...conv.messages];
        messages[msgIdx] = updated;
        const newConv = { ...conv, messages };
        const out = [...prev];
        out[idx] = newConv;
        return out;
      });
    });
    return () => { handler.then(un => un()); };
  }, []);

  // Tool-approval resolver — invoked by the inline ApprovalCard buttons.
  const resolveApproval = useCallback(async (callId: string, approved: boolean) => {
    try { await invoke("resolve_tool_approval", { callId, approved }); }
    catch (e) { setError("approval resolution failed: " + String(e)); }
  }, []);

  const handleIndexWorkspace = useCallback(async () => {
    setIndexProgress({ total: 0, indexed: 0, current: "starting...", nodes: 0, done: false });
    try { await invoke("index_workspace"); }
    catch (e) { setIndexProgress({ total: 0, indexed: 0, current: "", nodes: 0, done: true, error: String(e) }); }
  }, []);

  const handlePickWorkspace = useCallback(async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked !== "string") return;
      await invoke<WorkspaceInfo>("set_workspace", { path: picked });
      reloadWorkspaces(); setNeedsRestart(true);
    } catch (e) { setError("Workspace change failed: " + String(e)); }
  }, [reloadWorkspaces]);

  const handleSwitchWorkspace = useCallback(async (path: string) => {
    try {
      await invoke<WorkspaceInfo>("set_workspace", { path });
      reloadWorkspaces(); setNeedsRestart(true);
    } catch (e) { setError("Workspace switch failed: " + String(e)); }
  }, [reloadWorkspaces]);

  // ── Workflow execution ──────────────────────────────────────────────────
  const workflowTracesRef = useRef<Map<string, { convId: string; assistId: number }>>(new Map());

  const handleRunWorkflow = useCallback(async (workflowId: string, userInput: string) => {
    if (!activeId) return;
    const convId = activeId;
    const userMsg: Message = { id: uid(), role: "user", text: `/${workflowId}${userInput ? " " + userInput : ""}`, ts: Date.now() };
    const assistId = uid();
    const assistMsg: Message = { id: assistId, role: "assistant", text: "", streaming: true, events: [], ts: Date.now() };
    setConvs(p => p.map(c => c.id === convId ? { ...c, messages: [...c.messages, userMsg, assistMsg] } : c));
    setLoading(true); setError(null);
    try {
      const traceId = await invoke<string>("run_workflow", { workflowId, userInput });
      workflowTracesRef.current.set(traceId, { convId, assistId });
      setConvs(p => p.map(c => c.id === convId
        ? { ...c, messages: c.messages.map(m => m.id === assistId ? { ...m, workflowTraceId: traceId } : m) } : c));
    } catch (e) {
      setLoading(false); setError(String(e));
      setConvs(p => p.map(c => c.id === convId
        ? { ...c, messages: c.messages.map(m => m.id === assistId ? { ...m, streaming: false, error: String(e) } : m) } : c));
    }
  }, [activeId]);

  const handleExpandText = useCallback(async (commandId: string, args: string[]) => {
    return invoke<string>("expand_text_command", { commandId, args });
  }, []);

  const handleCancelWorkflow = useCallback(async (traceId: string) => {
    try { await invoke<boolean>("cancel_workflow", { traceId }); }
    catch (e) { console.error("cancel_workflow failed", e); }
  }, []);

  useEffect(() => {
    const sub = listen<WorkflowEventPayload>("workflow_event", e => {
      const { trace_id, event } = e.payload;
      const target = workflowTracesRef.current.get(trace_id);
      if (!target) return;
      const { convId, assistId } = target;
      setConvs(p => p.map(c => {
        if (c.id !== convId) return c;
        return { ...c, messages: c.messages.map(m => m.id !== assistId ? m : applyWorkflowEvent(m, event)) };
      }));
      if (event.type === "workflow_completed" || event.type === "workflow_cancelled") {
        workflowTracesRef.current.delete(trace_id);
        setLoading(false);
      }
    });
    return () => { sub.then(un => un()); };
  }, []);

  // ── Slash exec ──────────────────────────────────────────────────────────
  const execSlash = useCallback((cmd: string) => {
    if (cmd === "clear" || cmd === "reset" || cmd === "index" || cmd === "help") {
      setDraft("");
      if (inputRef.current) inputRef.current.style.height = "auto";
      if (cmd === "clear") { handleClearConv(); return; }
      if (cmd === "reset") { handleClearConv(); handleSend("__reset__internal"); return; }
      if (cmd === "index") { handleIndexWorkspace(); return; }
      if (cmd === "help") { handleSend("/help"); return; }
    }
    const item = allSlashItems.find(s => s.cmd === cmd);
    if (!item) return;
    if (item.kind === "workflow") {
      const tail = draft.replace(/^\s*\/\S+\s*/, "");
      setDraft(""); if (inputRef.current) inputRef.current.style.height = "auto";
      handleRunWorkflow(item.workflow ?? cmd, tail);
      return;
    }
    if (item.kind === "text") {
      const tail = draft.replace(/^\s*\/\S+\s*/, "");
      const args = tail.length > 0 ? tail.split(/\s+/) : [];
      handleExpandText(cmd, args).then(expanded => {
        setDraft(expanded);
        setTimeout(() => { inputRef.current?.focus(); growTextarea(); }, 0);
      }).catch(console.error);
    }
  }, [draft, allSlashItems]);

  // ── Send ────────────────────────────────────────────────────────────────
  const handleSend = useCallback(async (text: string) => {
    if (text === "__reset__internal") {
      invoke("truncate_history", { fromIndex: 0, convId: activeId ?? undefined }).catch(console.error);
      return;
    }
    if (text === "/help") {
      const helpText = "**Available commands**\n\n- `/clear` - Clear conversation\n- `/reset` - Clear + wipe backend\n- `/index` - Index workspace\n- `/help` - Show help";
      const id = activeId ?? newUuid();
      const helpMsg: Message = { id: uid(), role: "assistant", text: helpText, ts: Date.now() };
      setConvs(p => p.map(c => c.id === id ? { ...c, messages: [...c.messages, helpMsg] } : c));
      return;
    }
    if (streamRef.current) { streamRef.current.unlisten(); streamRef.current = null; }
    thinkingStartRef.current = 0;

    let convId = activeId;
    if (!convId) {
      const id = newUuid();
      setConvs(p => [{ id, title: titleFrom(text), messages: [], ts: Date.now() }, ...p]);
      setActiveId(id); convId = id;
    }

    // Snapshot + clear attachments so the send is atomic and the next prompt
    // starts fresh.  Object URLs are revoked when the chip is removed; here
    // we leak them deliberately so the message bubble can still preview.
    const sentAttachments = attachments;
    setAttachments([]);

    const userMsg: Message = {
      id: uid(), role: "user", text,
      attachments: sentAttachments.length > 0 ? sentAttachments : undefined,
      ts: Date.now(),
    };
    setConvs(p => p.map(c => {
      if (c.id !== convId) return c;
      const isFirst = c.messages.length === 0;
      return { ...c, title: isFirst ? titleFrom(text) : c.title, messages: [...c.messages, userMsg] };
    }));

    const reqId = newUuid();
    const assistId = uid();
    const assistMsg: Message = { id: assistId, role: "assistant", text: "", thinking: "", streaming: true, ts: Date.now() };
    setConvs(p => p.map(c => c.id === convId ? { ...c, messages: [...c.messages, assistMsg] } : c));
    setLoading(true); setError(null);

    try {
      const unlisten = await listen<StreamChunk>("stream_chunk", (event) => {
        const chunk = event.payload;
        if (chunk.id !== reqId) return;

        if (chunk.done) {
          unlisten(); streamRef.current = null; setLoading(false);
          setConvs(p => p.map(c => ({ ...c,
            messages: c.messages.map(m => {
              if (m.id !== assistId) return m;
              const thinkingMs = thinkingStartRef.current > 0 ? Date.now() - thinkingStartRef.current : m.thinkingMs;
              const thinkingTokens = m.thinking ? Math.round(m.thinking.length / 4) : undefined;
              thinkingStartRef.current = 0;
              return { ...m, streaming: false, thinkingMs, thinkingTokens,
                error: chunk.error ?? undefined, inputTokens: chunk.input_tokens, outputTokens: chunk.output_tokens };
            }),
          })));
          if (chunk.error) setError(chunk.error);
          return;
        }

        setConvs(p => p.map(c => ({ ...c,
          messages: c.messages.map(m => {
            if (m.id !== assistId) return m;
            if (chunk.thinking && thinkingStartRef.current === 0) thinkingStartRef.current = Date.now();
            let thinkingMs = m.thinkingMs;
            if ((chunk.text || chunk.tool_call) && thinkingStartRef.current > 0) {
              thinkingMs = Date.now() - thinkingStartRef.current;
              thinkingStartRef.current = 0;
            }
            const newTool: ToolInvocation | null = chunk.tool_call
              ? { name: chunk.tool_call, args: chunk.tool_args, result: chunk.tool_result } : null;
            const events: AgentEvent[] = [...(m.events ?? [])];
            if (chunk.thinking) {
              const last = events[events.length - 1];
              if (last && last.kind === "thought") events[events.length - 1] = { kind: "thought", text: last.text + chunk.thinking };
              else events.push({ kind: "thought", text: chunk.thinking });
            }
            if (newTool) events.push({ kind: "tool", tool: newTool });
            return { ...m, text: m.text + (chunk.text || ""), thinking: chunk.thinking ? (m.thinking || "") + chunk.thinking : m.thinking,
              thinkingMs, toolCalls: newTool ? [...(m.toolCalls || []), newTool] : m.toolCalls, events };
          }),
        })));
      });

      streamRef.current = { unlisten };
      const activeAgent = activeId ? convs.find(c => c.id === activeId)?.agent_id : undefined;
      // Map UI drafts to the wire-format `Attachment` enum that
      // `owl-protocol::attachment::Attachment` expects (snake_case `kind`).
      const wireAttachments = sentAttachments.map(a => a.kind === "image"
        ? { kind: "image", mime_type: a.mime_type, data: a.data, alt_text: a.alt_text }
        : { kind: "text",  filename:  a.filename,  content: a.data });
      invoke("stream_message", {
        input: {
          id: reqId, message: text,
          agent_id: activeAgent, conv_id: convId,
          attachments: wireAttachments.length > 0 ? wireAttachments : undefined,
        }
      }).catch(e => {
        setError(String(e)); setLoading(false); unlisten(); streamRef.current = null;
        setConvs(p => p.map(c => ({ ...c, messages: c.messages.map(m => m.id === assistId ? { ...m, streaming: false } : m) })));
      });
    } catch (e) {
      setError(String(e)); setLoading(false);
      setConvs(p => p.map(c => ({ ...c, messages: c.messages.map(m => m.id === assistId ? { ...m, streaming: false } : m) })));
    }
  }, [activeId, convs, attachments]);

  const handleRegenerate = useCallback(async () => {
    if (loading || !activeId) return;
    const conv = convs.find(c => c.id === activeId);
    if (!conv) return;
    const lastUserMsg = [...conv.messages].reverse().find(m => m.role === "user");
    if (!lastUserMsg) return;
    const userIdx = conv.messages.findIndex(m => m.id === lastUserMsg.id);
    if (userIdx < 0) return;
    setConvs(p => p.map(c => c.id === activeId ? { ...c, messages: c.messages.slice(0, userIdx) } : c));
    try { await invoke("truncate_history", { fromIndex: userIdx, convId: activeId }); }
    catch (e) { setError("Truncate failed: " + String(e)); return; }
    handleSend(lastUserMsg.text);
  }, [loading, activeId, convs, handleSend]);

  const handleEditMessage = useCallback(async (msgId: number, _newDraft: string) => {
    if (loading || !activeId) return;
    const conv = convs.find(c => c.id === activeId);
    if (!conv) return;
    const idx = conv.messages.findIndex(m => m.id === msgId);
    if (idx < 0) return;
    setConvs(p => p.map(c => c.id === activeId ? { ...c, messages: c.messages.slice(0, idx) } : c));
    try { await invoke("truncate_history", { fromIndex: idx, convId: activeId }); }
    catch (e) { setError("Truncate failed: " + String(e)); }
  }, [loading, activeId, convs]);

  const handleClearConv = useCallback(() => {
    if (!activeId) return;
    setConvs(p => p.map(c => c.id === activeId ? { ...c, messages: [] } : c));
    invoke("truncate_history", { fromIndex: 0, convId: activeId }).catch(console.error);
  }, [activeId]);

  const handleRenameConv = useCallback((id: string, title: string) => {
    setConvs(p => p.map(c => c.id === id ? { ...c, title } : c));
  }, []);

  const handleDeleteConv = useCallback((id: string) => {
    setConvs(p => {
      const next = p.filter(c => c.id !== id);
      if (activeId === id) setActiveId(next[0]?.id ?? null);
      return next;
    });
  }, [activeId]);

  const handleStop = useCallback(() => {
    if (streamRef.current) { streamRef.current.unlisten(); streamRef.current = null; }
    setLoading(false);
    setConvs(p => p.map(c => ({ ...c, messages: c.messages.map(m => m.streaming ? { ...m, streaming: false } : m) })));
  }, []);

  // Per-message actions wired through to UserBubble + AssistantBubble.
  const handleDeleteMessage = useCallback((id: number) => {
    if (!activeId) return;
    setConvs(p => p.map(c => c.id === activeId
      ? { ...c, messages: c.messages.filter(m => m.id !== id) }
      : c));
  }, [activeId]);

  const handleReplyTo = useCallback((msg: Message) => {
    // Quote the message into the composer with `> …` prefix lines.
    const quoted = msg.text.split("\n").map(l => `> ${l}`).join("\n");
    setDraft(d => (d ? `${d}\n\n${quoted}\n` : `${quoted}\n`));
    setTimeout(() => {
      inputRef.current?.focus();
      const el = inputRef.current; if (el) el.selectionStart = el.selectionEnd = el.value.length;
    }, 0);
  }, []);

  const handleBranchFrom = useCallback((id: number) => {
    if (!activeId) return;
    const conv = convs.find(c => c.id === activeId);
    if (!conv) return;
    const cutIdx = conv.messages.findIndex(m => m.id === id);
    if (cutIdx < 0) return;
    const branched: Conversation = {
      id: newUuid(),
      title: `🌿 ${conv.title}`,
      messages: conv.messages.slice(0, cutIdx + 1).map(m => ({ ...m })),
      ts: Date.now(),
      agent_id: conv.agent_id,
    };
    setConvs(p => [branched, ...p]);
    setActiveId(branched.id);
  }, [activeId, convs]);

  const handleReact = useCallback((id: number, reaction: "up" | "down") => {
    if (!activeId) return;
    setConvs(p => p.map(c => c.id === activeId
      ? { ...c, messages: c.messages.map(m => m.id === id
          ? { ...m, reaction: m.reaction === reaction ? undefined : reaction }
          : m) }
      : c));
  }, [activeId]);

  // ── Composer attachments (drag-drop / paste / file picker) ─────────────
  const fileToAttachment = useCallback(async (file: File): Promise<AttachmentDraft | null> => {
    const id = newUuid();
    if (file.type.startsWith("image/")) {
      const buf = await file.arrayBuffer();
      const data = btoa(String.fromCharCode(...new Uint8Array(buf)));
      return {
        id, kind: "image", mime_type: file.type, data,
        preview: URL.createObjectURL(file),
        alt_text: file.name,
      };
    }
    // Treat anything else as text — best-effort UTF-8 decode.
    if (file.size > 1024 * 256) {
      setError(`Attachment "${file.name}" exceeds 256 KB text-attachment limit`);
      return null;
    }
    const text = await file.text();
    return { id, kind: "text", filename: file.name, data: text };
  }, []);

  const addAttachments = useCallback(async (files: FileList | File[]) => {
    const arr = Array.from(files);
    const drafts = await Promise.all(arr.map(fileToAttachment));
    setAttachments(prev => [...prev, ...drafts.filter((d): d is AttachmentDraft => d != null)]);
  }, [fileToAttachment]);

  const removeAttachment = useCallback((id: string) => {
    setAttachments(prev => {
      const a = prev.find(x => x.id === id);
      if (a?.preview) URL.revokeObjectURL(a.preview);
      return prev.filter(x => x.id !== id);
    });
  }, []);

  // ── Keyboard shortcuts ────────────────────────────────────────────────
  useEffect(() => {
    const handler = (e: globalThis.KeyboardEvent) => {
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.key === "k") { e.preventDefault(); newConv(); }
      if (meta && e.key === "l") { e.preventDefault(); inputRef.current?.focus(); }
      if (meta && e.key === "b") { e.preventDefault(); setSidebarOpen(o => !o); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [newConv]);

  // Auto-scroll
  const isNearBottom = () => {
    const el = listRef.current; if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };
  useEffect(() => {
    if (atBottom) listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: "smooth" });
  }, [activeConv?.messages.length, loading, atBottom]);
  useEffect(() => { setTimeout(() => inputRef.current?.focus(), 50); }, [activeId]);

  const hasStreaming = activeConv?.messages.some(m => m.streaming) ?? false;
  useEffect(() => { if (hasStreaming && atBottom) listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: "smooth" }); });

  const growTextarea = () => {
    const el = inputRef.current; if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 200) + "px";
  };

  const send = useCallback(() => {
    const t = draft.trim();
    if (!t || loading) return;
    setDraft(""); if (inputRef.current) inputRef.current.style.height = "auto";
    handleSend(t);
  }, [draft, loading, handleSend]);

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (slashOpen && slashCmds.length > 0) {
      if (e.key === "ArrowUp") { e.preventDefault(); setSlashIdx(i => Math.max(0, i - 1)); return; }
      if (e.key === "ArrowDown") { e.preventDefault(); setSlashIdx(i => Math.min(slashCmds.length - 1, i + 1)); return; }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault(); execSlash(slashCmds[Math.min(slashIdx, slashCmds.length - 1)]?.cmd ?? ""); return;
      }
      if (e.key === "Escape") { setDraft(""); return; }
    }
    if (e.key === "ArrowUp" && draft === "" && !loading) {
      const lastUser = activeConv?.messages.slice().reverse().find(m => m.role === "user");
      if (lastUser) { e.preventDefault(); setDraft(lastUser.text); setTimeout(growTextarea, 0); return; }
    }
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
  };

  const lastAssistantId = activeConv?.messages.slice().reverse().find(m => m.role === "assistant" && !m.streaming)?.id;
  const lastUserId = activeConv?.messages.slice().reverse().find(m => m.role === "user")?.id;
  const handleEdit = (msgId: number, currentText: string) => {
    setDraft(currentText);
    setTimeout(() => { const el = inputRef.current; if (el) { el.focus(); el.setSelectionRange(currentText.length, currentText.length); growTextarea(); } }, 0);
    handleEditMessage(msgId, currentText);
  };

  // File drop — images become attachments (sent via vision API), text files
  // become attachment chips (model gets them as Attachment::Text blocks).
  const [dragOver, setDragOver] = useState(false);
  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault(); setDragOver(false);
    if (e.dataTransfer.files.length > 0) {
      addAttachments(e.dataTransfer.files);
    }
  }, [addAttachments]);

  // Paste handler — capture screenshot/image from clipboard (Cmd+V on an image).
  const handlePaste = useCallback((e: React.ClipboardEvent) => {
    const files: File[] = [];
    for (const item of Array.from(e.clipboardData.items)) {
      if (item.kind === "file") {
        const f = item.getAsFile();
        if (f) files.push(f);
      }
    }
    if (files.length > 0) {
      e.preventDefault();
      addAttachments(files);
    }
  }, [addAttachments]);

  // Grouped conversations
  const groupedConvs = useMemo(() => {
    const tod = new Date().setHours(0, 0, 0, 0);
    const yest = tod - 86_400_000;
    const week = tod - 6 * 86_400_000;
    const groups: Array<{ label: string; items: Conversation[] }> = [
      { label: "Today", items: [] }, { label: "Yesterday", items: [] },
      { label: "Last 7 days", items: [] }, { label: "Older", items: [] },
    ];
    for (const c of convs) {
      const ts = c.ts ?? c.messages[c.messages.length - 1]?.ts ?? Date.now();
      if (ts >= tod) groups[0].items.push(c);
      else if (ts >= yest) groups[1].items.push(c);
      else if (ts >= week) groups[2].items.push(c);
      else groups[3].items.push(c);
    }
    return groups.filter(g => g.items.length > 0);
  }, [convs]);

  // Context menu
  const [ctxMenu, setCtxMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  useEffect(() => {
    if (!ctxMenu) return;
    const close = () => setCtxMenu(null);
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [ctxMenu]);

  // Rename
  const [renamingId, setRenamingId] = useState<string|null>(null);
  const [renameVal, setRenameVal] = useState("");
  const startRename = (c: Conversation) => { setRenamingId(c.id); setRenameVal(c.title); };
  const commitRename = () => {
    if (renamingId && renameVal.trim()) handleRenameConv(renamingId, renameVal.trim());
    setRenamingId(null);
  };

  // Token usage for footer
  const usage = activeConv ? convUsage(activeConv) : { input: 0, output: 0, estimated: true };
  const totalTok = usage.input + usage.output;
  const cost = estCostUsd(usage.input, usage.output);

  const hasContent = draft.trim().length > 0;
  const indexing = indexProgress && !indexProgress.done;

  const togglePanel = (panel: SidePanel) => {
    setSidePanel(p => p === panel ? null : panel);
  };

  return (
    <div style={sty.root}>
      {/* Owl chibi pet — floating, draggable; survives across panels. */}
      <OwlPetWidget/>

      {/* ═══ LEFT SIDEBAR ═══ */}
      {sidebarOpen && (
        <aside style={sty.sidebar}>
          {/* Logo */}
          <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "14px 14px 10px" }}>
            <div style={{ width: 28, height: 28, borderRadius: 7, background: C.accentBg, border: `1px solid ${C.accent}44`,
              display: "flex", alignItems: "center", justifyContent: "center", color: C.accent, flexShrink: 0 }}><IcOwl/></div>
            <span style={{ fontWeight: 700, fontSize: 13, color: C.text }}>Knight Owl</span>
          </div>
          {/* New conversation */}
          <button style={sty.newConvBtn} onClick={newConv}><IcPlus/> New chat</button>
          {/* Conversation list */}
          <div style={{ flex: 1, overflowY: "auto", padding: "0 6px" }}>
            {convs.length === 0 && <p style={{ color: C.muted, fontSize: 11, padding: "8px 10px" }}>No conversations</p>}
            {groupedConvs.map(group => (
              <div key={group.label}>
                <div style={sty.convGroupLabel}>{group.label}</div>
                {group.items.map(c => (
                  <div key={c.id} style={{ position: "relative" as const }}>
                    {renamingId === c.id
                      ? <input autoFocus value={renameVal} onChange={e => setRenameVal(e.target.value)}
                          onBlur={commitRename} onKeyDown={e => { if (e.key === "Enter") commitRename(); if (e.key === "Escape") setRenamingId(null); }}
                          style={{ ...sty.convItem, ...sty.convItemActive, display: "block", width: "100%",
                            background: "#ffffff18", border: `1px solid ${C.accent}66`, outline: "none", boxSizing: "border-box" as const }}/>
                      : <button style={{ ...sty.convItem, ...(c.id === activeId ? sty.convItemActive : {}) }}
                          onClick={() => { setActiveId(c.id); setError(null); }}
                          onDoubleClick={() => startRename(c)}
                          onContextMenu={e => { e.preventDefault(); setCtxMenu({ id: c.id, x: e.clientX, y: e.clientY }); }}>
                          {c.title}
                        </button>
                    }
                  </div>
                ))}
              </div>
            ))}
          </div>
          {/* Workspace + footer */}
          <div style={{ padding: "8px 10px", borderTop: `1px solid ${C.border}`, flexShrink: 0 }}>
            <button style={sty.workspaceBtn} onClick={handlePickWorkspace}
              title={workspace ? workspace.path : "Pick a workspace"}>
              <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {workspace?.name ?? "No workspace"}
              </span>
            </button>
            {indexing && (
              <div style={{ height: 3, background: C.border, borderRadius: 99, overflow: "hidden", marginBottom: 6 }}>
                <div style={{ width: `${indexProgress!.total > 0 ? Math.round((indexProgress!.indexed / indexProgress!.total) * 100) : 0}%`,
                  height: "100%", background: C.accent, transition: "width 0.2s" }}/>
              </div>
            )}
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: 10, color: C.muted }}>
              <span>gemini-2.5-flash</span>
              {totalTok > 0 && <span>{usage.estimated ? "~" : ""}{formatK(totalTok)} tok . {formatCost(cost)}</span>}
            </div>
          </div>
        </aside>
      )}

      {/* Context menu */}
      {ctxMenu && (
        <div style={{ ...sty.ctxMenu, position: "fixed" as const, top: ctxMenu.y, left: ctxMenu.x, zIndex: 300 }}
          onMouseDown={e => e.stopPropagation()}>
          <button style={sty.ctxItem} onClick={() => { startRename(convs.find(c => c.id === ctxMenu.id)!); setCtxMenu(null); }}>Rename</button>
          <button style={sty.ctxItem} onClick={() => {
            const conv = convs.find(c => c.id === ctxMenu.id);
            if (conv) navigator.clipboard.writeText(convToMarkdown(conv));
            setCtxMenu(null);
          }}>Copy all</button>
          <button style={sty.ctxItem} onClick={() => {
            const conv = convs.find(c => c.id === ctxMenu.id);
            if (conv) { const b = new Blob([convToMarkdown(conv)], { type: "text/markdown" }); const a = document.createElement("a"); a.href = URL.createObjectURL(b); a.download = `${conv.title.replace(/[^a-z0-9]/gi, "_")}.md`; a.click(); }
            setCtxMenu(null);
          }}>Export</button>
          <div style={{ height: 1, background: C.border, margin: "3px 0" }}/>
          <button style={{ ...sty.ctxItem, color: "#f87171" }} onClick={() => { handleDeleteConv(ctxMenu.id); setCtxMenu(null); }}>Delete</button>
        </div>
      )}

      {/* ═══ CENTER: CHAT ═══ */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minWidth: 0 }}>
        {/* Top bar */}
        <div style={sty.topBar}>
          <button style={sty.topBarBtn} onClick={() => setSidebarOpen(o => !o)} title="Toggle sidebar (Cmd+B)">
            <IcSidebar/>
          </button>
          <span style={{ flex: 1, fontSize: 13, fontWeight: 600, color: C.text, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {activeConv ? activeConv.title : "New conversation"}
          </span>
          {loading && <span style={{ fontSize: 11, color: C.accent, background: C.accentBg, padding: "2px 8px", borderRadius: 99 }}>thinking...</span>}
          {/* Agent picker */}
          <select style={sty.agentPicker} value={activeConv?.agent_id ?? ""} disabled={!activeConv || loading}
            onChange={e => handleSetAgent(e.target.value || null)}>
            <option value="">Default</option>
            {agents.map(a => <option key={a.id} value={a.id}>{a.name}</option>)}
          </select>
          {/* Side panel toggles */}
          <button style={{ ...sty.topBarBtn, ...(sidePanel === "knowledge" ? { color: C.accent } : {}) }}
            onClick={() => togglePanel("knowledge")} title="Knowledge Base"><IcGraph/></button>
          <button style={{ ...sty.topBarBtn, ...(sidePanel === "agents" ? { color: C.accent } : {}) }}
            onClick={() => togglePanel("agents")} title="Agents"><IcAgents/></button>
          <button style={{ ...sty.topBarBtn, ...(sidePanel === "tools" ? { color: C.accent } : {}) }}
            onClick={() => togglePanel("tools")} title="Tools & MCP"><IcTools/></button>
          <button style={{ ...sty.topBarBtn, ...(sidePanel === "settings" ? { color: C.accent } : {}) }}
            onClick={() => togglePanel("settings")} title="Settings"><IcGear/></button>
          <button style={sty.topBarBtn} title="Show pet"
            onClick={() => {
              localStorage.removeItem("owl-pet-hidden");
              window.dispatchEvent(new Event("owl-pet-show"));
            }}>🦉</button>
        </div>

        {needsRestart && (
          <div style={{ padding: "8px 20px", background: "#1f1d0e", borderBottom: "1px solid #4a3c18", color: "#fcd34d", fontSize: 12,
            display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <span>Workspace changed to <b>{workspace?.name}</b>. Restart to apply.</span>
            <button onClick={() => setNeedsRestart(false)} style={{ background: "none", border: "none", color: "#fcd34d", cursor: "pointer" }}>x</button>
          </div>
        )}

        {/* Messages */}
        <div ref={listRef} style={sty.messageList} onScroll={() => setAtBottom(isNearBottom())}>
          {(!activeConv || activeConv.messages.length === 0) && !loading
            ? <EmptyChat onPick={text => handleSend(text)}/>
            : activeConv?.messages.map((m, i) => {
              const prev = i > 0 ? activeConv.messages[i-1] : null;
              const roleChanged = prev && prev.role !== m.role;
              return (
                <React.Fragment key={m.id}>
                  {roleChanged && <div style={{ height: 1, background: `${C.border}55`, margin: "4px 48px" }}/>}
                  {m.role === "user"
                    ? <UserBubble msg={m}
                        onEdit={m.id === lastUserId && !loading ? () => handleEdit(m.id, m.text) : undefined}
                        onDelete={!loading ? () => handleDeleteMessage(m.id) : undefined}
                        onReply={!loading ? () => handleReplyTo(m) : undefined}
                        onBranch={!loading ? () => handleBranchFrom(m.id) : undefined}/>
                    : <AssistantBubble msg={m} loading={loading && m.streaming}
                        isLast={m.id === lastAssistantId}
                        onRegenerate={m.id === lastAssistantId && !loading ? handleRegenerate : undefined}
                        onCancelWorkflow={handleCancelWorkflow}
                        onResolveApproval={resolveApproval}
                        onReact={!loading ? r => handleReact(m.id, r) : undefined}
                        onBranch={!loading ? () => handleBranchFrom(m.id) : undefined}
                        onDelete={!loading ? () => handleDeleteMessage(m.id) : undefined}/>
                  }
                </React.Fragment>
              );
            })
          }
          {loading && !(activeConv?.messages.some(m => m.role === "assistant" && m.streaming)) && (
            <div style={sty.assistantRow}>
              <div style={{ ...sty.avatar, ...sty.avatarPulse }}><IcOwl/></div>
              <div style={{ paddingTop: 6 }}><TypingDots/></div>
            </div>
          )}
        </div>

        {/* Scroll to bottom */}
        {!atBottom && (activeConv?.messages.length ?? 0) > 0 && (
          <button style={sty.scrollBtn} onClick={() => {
            listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: "smooth" });
            setAtBottom(true);
          }}><IcArrowDown/></button>
        )}

        {/* Error bar */}
        {error && (
          <div style={sty.errorBar}>
            <span style={{ flex: 1, fontSize: 12, color: "#fca5a5" }}>{error}</span>
            <button style={sty.retryBtn} onClick={handleRegenerate}>Retry</button>
            <button style={{ background: "none", border: "none", color: "#fca5a5", cursor: "pointer", fontSize: 14 }}
              onClick={() => setError(null)}>x</button>
          </div>
        )}

        {/* Input area */}
        <div style={{ ...sty.inputArea, position: "relative" as const }}
          onDragOver={e => { e.preventDefault(); setDragOver(true); }}
          onDragLeave={() => setDragOver(false)} onDrop={handleDrop}>
          {slashOpen && slashCmds.length > 0 && (
            <SlashMenu items={slashCmds} filter={slashFilter ?? ""} selectedIdx={Math.min(slashIdx, slashCmds.length - 1)} onSelect={execSlash}/>
          )}
          {dragOver && (
            <div style={sty.dropOverlay}>
              <div style={{ fontSize: 14, color: C.accent }}>Drop images / files to attach</div>
            </div>
          )}
          {loading && (
            <button style={sty.stopBigBtn} onClick={handleStop} title="Stop generation (Esc)">
              <IcStop/> Stop
            </button>
          )}
          {/* Attachment chip strip — shown above the textarea while drafting. */}
          {attachments.length > 0 && (
            <div style={sty.attachmentStrip}>
              {attachments.map(a => (
                <div key={a.id} style={sty.attachmentChip}>
                  {a.kind === "image"
                    ? <img src={a.preview} alt="" style={{ width: 28, height: 28, objectFit: "cover", borderRadius: 4 }}/>
                    : <span style={{ fontSize: 14 }}>📄</span>}
                  <span style={{ fontSize: 11.5, maxWidth: 120, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {a.kind === "image" ? (a.alt_text ?? "image") : a.filename}
                  </span>
                  <button style={sty.chipClose} onClick={() => removeAttachment(a.id)} title="Remove">×</button>
                </div>
              ))}
            </div>
          )}
          <div style={{ ...sty.inputBox, ...(loading ? { opacity: 0.7 } : {}),
            ...(dragOver ? { borderColor: C.accent } : {}) }}>
            <button style={sty.attachBtn} title="Attach files / images" disabled={loading}
              onClick={async () => {
                const picked = await openDialog({
                  multiple: true,
                  filters: [{ name: "Image", extensions: ["png","jpg","jpeg","gif","webp"] }],
                });
                if (!picked) return;
                const paths = Array.isArray(picked) ? picked : [picked];
                // Read each file as a binary blob via fetch (Tauri serves file:// URLs).
                const files: File[] = await Promise.all(paths.map(async p => {
                  const r = await fetch(`file://${p}`); const b = await r.blob();
                  return new File([b], p.split("/").pop() ?? "file", { type: b.type });
                }));
                addAttachments(files);
              }}>📎</button>
            <textarea ref={inputRef} style={sty.textarea} value={draft} rows={1}
              disabled={loading} placeholder="Message Knight Owl..."
              onChange={e => { setDraft(e.target.value); growTextarea(); }}
              onKeyDown={onKeyDown}
              onPaste={handlePaste}/>
            {loading
              ? <button style={{ ...sty.sendBtn, background: "#3f3f4e" }} onClick={handleStop} title="Stop"><IcStop/></button>
              : <button style={{ ...sty.sendBtn, ...(hasContent ? {} : { background: "#2a2a38", color: C.muted, cursor: "default" }) }}
                  disabled={!hasContent} onClick={send} title="Send"><IcSend/></button>
            }
          </div>
          <p style={{ margin: "4px 0 0", fontSize: 11, color: C.muted, textAlign: "center" }}>
            Enter to send · Shift+Enter newline · <span style={{ color: C.accent }}>/</span> commands · drop / paste images for vision
          </p>
        </div>
      </div>

      {/* ═══ RIGHT SIDE PANEL ═══ */}
      {sidePanel && (
        <div style={sty.sidePanel}>
          <div style={{ display: "flex", alignItems: "center", padding: "10px 14px", borderBottom: `1px solid ${C.border}`, gap: 8 }}>
            <span style={{ flex: 1, fontSize: 13, fontWeight: 600, color: C.text }}>
              {sidePanel === "knowledge" ? "Knowledge Base" : sidePanel === "tools" ? "Tools & MCP" : sidePanel === "agents" ? "Agents" : "Settings"}
            </span>
            <button style={sty.topBarBtn} onClick={() => setSidePanel(null)}><IcClose/></button>
          </div>
          <div style={{ flex: 1, overflow: "hidden" }}>
            {sidePanel === "knowledge" && <KnowledgeBasePanel/>}
            {sidePanel === "tools" && <ToolsPanel/>}
            {sidePanel === "agents" && <AgentsPanel/>}
            {sidePanel === "settings" && <SettingsPanel/>}
          </div>
        </div>
      )}
    </div>
  );
}

// ─── Styles ───────────────────────────────────────────────────────────────────
const FONT = "-apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', sans-serif";
const MONO = "'JetBrains Mono','Fira Code','Menlo',monospace";

const sty: Record<string, React.CSSProperties> = {
  root: { display: "flex", height: "100vh", background: C.bg, color: C.text, overflow: "hidden", fontFamily: FONT },

  // Sidebar
  sidebar: { width: 260, minWidth: 260, background: C.sidebar, borderRight: `1px solid ${C.border}`, display: "flex", flexDirection: "column", overflow: "hidden" },
  newConvBtn: { display: "flex", alignItems: "center", gap: 6, margin: "0 10px 6px", padding: "8px 10px", background: "transparent",
    border: `1px solid ${C.border}`, borderRadius: 8, color: C.muted, fontSize: 12, cursor: "pointer" },
  convGroupLabel: { fontSize: 10, fontWeight: 700, color: C.muted, opacity: 0.6, letterSpacing: "0.06em",
    textTransform: "uppercase" as const, padding: "10px 10px 3px", userSelect: "none" as const },
  convItem: { display: "block", width: "100%", padding: "7px 10px", marginBottom: 1, background: "transparent", border: "none",
    borderRadius: 6, color: C.muted, fontSize: 12, cursor: "pointer", textAlign: "left" as const,
    overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  convItemActive: { background: "#ffffff10", color: C.text },
  workspaceBtn: { display: "flex", alignItems: "center", gap: 6, width: "100%", padding: "6px 8px",
    background: C.surface, border: `1px solid ${C.border}`, borderRadius: 6, color: C.text, fontSize: 11, cursor: "pointer", marginBottom: 6 },

  // Top bar
  topBar: { display: "flex", alignItems: "center", gap: 6, padding: "8px 16px", borderBottom: `1px solid ${C.border}`, flexShrink: 0, background: C.bg },
  topBarBtn: { background: "none", border: "none", color: C.muted, cursor: "pointer", padding: "4px 6px", borderRadius: 5, display: "flex", alignItems: "center" },
  agentPicker: { fontSize: 11, color: C.text, background: C.surface, border: `1px solid ${C.border}`, borderRadius: 6,
    padding: "3px 8px", cursor: "pointer", fontFamily: "inherit", outline: "none" },

  // Chat
  messageList: { flex: 1, overflowY: "auto", padding: "16px 0", scrollbarWidth: "thin", scrollbarColor: `${C.border} transparent` },
  userRow: { display: "flex", gap: 12, padding: "12px 24px", maxWidth: 800, margin: "0 auto", width: "100%", boxSizing: "border-box" as const },
  assistantRow: { display: "flex", gap: 12, padding: "12px 24px", maxWidth: 800, margin: "0 auto", width: "100%", boxSizing: "border-box" as const },
  avatar: { width: 28, height: 28, borderRadius: "50%", flexShrink: 0, background: C.accentBg, border: `1px solid ${C.accent}44`,
    display: "flex", alignItems: "center", justifyContent: "center", color: C.accent, marginTop: 2 },
  avatarPulse: { animation: "avatarPulse 1.6s ease-in-out infinite" },

  // Input
  inputArea: { padding: "8px 24px 12px", maxWidth: 800, margin: "0 auto", width: "100%", boxSizing: "border-box" as const, flexShrink: 0 },
  inputBox: { background: C.inputBg, border: `1px solid ${C.border}`, borderRadius: 14, display: "flex", alignItems: "flex-end",
    gap: 8, padding: "9px 10px 9px 14px", boxShadow: "0 2px 12px rgba(0,0,0,0.2)" },
  textarea: { flex: 1, background: "transparent", border: "none", outline: "none", resize: "none", color: C.text, fontSize: 14,
    lineHeight: 1.6, fontFamily: "inherit", padding: "2px 0", minHeight: 24, maxHeight: 200, overflowY: "auto" },
  sendBtn: { width: 32, height: 32, borderRadius: 7, border: "none", background: C.accent, color: "#fff",
    display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer", flexShrink: 0 },

  // Side panel
  sidePanel: { width: 380, minWidth: 380, background: C.sidebar, borderLeft: `1px solid ${C.border}`,
    display: "flex", flexDirection: "column", overflow: "hidden" },

  // Errors
  errorBar: { display: "flex", alignItems: "center", gap: 8, padding: "8px 24px", background: "#1f0e0e", borderTop: "1px solid #4a1818", flexShrink: 0 },
  inlineError: { display: "flex", alignItems: "center", gap: 8, marginTop: 8, padding: "8px 12px", background: "#1f0e0e", border: "1px solid #4a1818", borderRadius: 8, color: "#fca5a5" },
  retryBtn: { fontSize: 11, color: "#fca5a5", background: "#2a0e0e", border: "1px solid #6a2020", borderRadius: 6, padding: "3px 8px", cursor: "pointer", fontWeight: 600 },
  cancelBtn: { display: "inline-flex", alignItems: "center", gap: 6, margin: "6px 0", padding: "4px 10px", fontSize: 11,
    background: "#3f1d1d", color: "#fca5a5", border: "1px solid #6a2020", borderRadius: 6, cursor: "pointer" },
  scrollBtn: { position: "absolute" as const, bottom: 90, right: 24, width: 34, height: 34, borderRadius: "50%",
    background: C.surface, border: `1px solid ${C.border}`, color: C.text, cursor: "pointer",
    display: "flex", alignItems: "center", justifyContent: "center", boxShadow: "0 4px 14px rgba(0,0,0,0.4)", zIndex: 10 },

  // Thinking / activity
  thinkingWrap: { margin: "6px 0 10px", borderRadius: 8, border: `1px solid ${C.border}`, overflow: "hidden" },
  thinkingToggle: { display: "flex", alignItems: "center", justifyContent: "space-between", width: "100%", padding: "7px 12px",
    background: "#1a1a28", border: "none", cursor: "pointer", color: C.muted, fontSize: 12, gap: 6 },
  thinkingBody: { background: "#12121a", borderTop: `1px solid ${C.border}` },

  // Tool chip
  toolChip: { display: "inline-flex", alignItems: "center", gap: 6, fontSize: 11.5, color: "#60a5fa", background: "#1a2238",
    border: "1px solid #1e40af40", borderRadius: 99, padding: "3px 10px", cursor: "pointer" },
  toolDetail: { width: "100%", marginTop: 4, background: "#0f1118", border: `1px solid ${C.border}`, borderRadius: 8, padding: "8px 12px", fontSize: 11.5 },
  toolDetailLabel: { fontSize: 10, color: C.muted, fontWeight: 700, textTransform: "uppercase" as const, marginBottom: 3 },
  toolDetailPre: { margin: "0 0 6px", padding: 0, background: "transparent", color: "#c8d3e8", fontFamily: MONO,
    fontSize: 11, whiteSpace: "pre-wrap" as const, wordBreak: "break-word" as const, maxHeight: 200, overflowY: "auto" as const },

  // Code
  copyBtn: { background: "#2a2a38", border: `1px solid ${C.border}`, borderRadius: 5, color: C.muted, fontSize: 10.5, padding: "3px 8px", cursor: "pointer" },
  codeInner: { fontSize: 12.5, fontFamily: MONO, color: "#c8d3e8", background: "transparent" },
  inlineCode: { background: C.code, border: `1px solid ${C.border}`, borderRadius: 4, padding: "1px 5px", fontSize: "0.88em", fontFamily: MONO, color: "#c8d3e8" },
  collapseBtn: { display: "block", width: "100%", padding: "5px 0", textAlign: "center" as const, background: "#0b0b12",
    border: `1px solid ${C.border}`, borderTop: "none", borderRadius: "0 0 9px 9px", color: C.muted, fontSize: 11, cursor: "pointer" },

  // Markdown
  mdTable: { borderCollapse: "collapse" as const, width: "100%", fontSize: 13 },
  mdTh: { padding: "8px 12px", textAlign: "left" as const, fontWeight: 600, color: C.text, background: "#1c1c26", borderBottom: `2px solid ${C.border}` },
  mdTd: { padding: "7px 12px", color: C.text, verticalAlign: "top" as const },
  mdBlockquote: { margin: "10px 0", padding: "2px 0 2px 14px", borderLeft: `3px solid ${C.accent}88`, color: C.muted, fontStyle: "italic" },
  mdH: { color: C.text, fontWeight: 700, margin: "0 0 8px" },
  mdList: { paddingLeft: 22, margin: "4px 0 10px" },

  // Buttons
  miniBtn: { background: C.surface, border: `1px solid ${C.border}`, borderRadius: 5, color: C.muted, fontSize: 10, padding: "2px 6px", cursor: "pointer" },
  btnPrimary: { display: "inline-flex", alignItems: "center", gap: 6, padding: "6px 12px", background: C.accent, border: "none",
    borderRadius: 7, color: "#fff", fontSize: 12, fontWeight: 600, cursor: "pointer" },
  btnGhost: { display: "inline-flex", alignItems: "center", gap: 6, padding: "6px 12px", background: "transparent",
    border: `1px solid ${C.border}`, borderRadius: 7, color: C.text, fontSize: 12, cursor: "pointer" },

  // Examples
  exampleCard: { padding: "12px 14px", textAlign: "left" as const, fontSize: 13, lineHeight: 1.45,
    background: C.surface, color: C.text, border: `1px solid ${C.border}`, borderRadius: 10, cursor: "pointer" },

  // Slash menu
  slashMenu: { position: "absolute" as const, bottom: "calc(100% + 6px)", left: 0, right: 0,
    background: C.surface, border: `1px solid ${C.border}`, borderRadius: 10,
    boxShadow: "0 -8px 28px rgba(0,0,0,0.45)", overflow: "hidden", zIndex: 100 },
  slashItem: { display: "flex", alignItems: "center", gap: 10, width: "100%", padding: "9px 14px",
    background: "transparent", border: "none", color: C.text, fontSize: 13, cursor: "pointer", textAlign: "left" as const },
  slashItemActive: { background: `${C.accent}18` },

  // Context menu
  ctxMenu: { background: C.surface, border: `1px solid ${C.border}`, borderRadius: 8, boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
    overflow: "hidden", minWidth: 140, padding: "4px 0" },
  ctxItem: { display: "flex", width: "100%", padding: "7px 14px", background: "transparent", border: "none",
    color: C.text, fontSize: 12, cursor: "pointer", textAlign: "left" as const },

  // Cards
  card: { background: C.surface, border: `1px solid ${C.border}`, borderRadius: 8, padding: "10px 12px", cursor: "pointer" },

  // Side panel sub-components
  panelInput: { flex: 1, background: C.inputBg, border: `1px solid ${C.border}`, borderRadius: 6, padding: "5px 8px",
    color: C.text, fontSize: 12, outline: "none", fontFamily: "inherit" },
  panelBtn: { background: C.accent, color: "#fff", border: "none", borderRadius: 6, padding: "5px 12px", fontSize: 11, cursor: "pointer", fontWeight: 600 },
  panelTab: { padding: "6px 10px", fontSize: 11, cursor: "pointer", color: C.muted, background: "none",
    border: "none", borderBottom: "2px solid transparent" },
  panelListItem: { display: "flex", alignItems: "center", gap: 6, padding: "5px 14px", fontSize: 11, cursor: "pointer",
    color: C.muted, borderRadius: 4, margin: "0 4px" },

  // Forms
  formLabel: { display: "flex", flexDirection: "column" as const, gap: 4, fontSize: 12, color: C.muted },

  // Modal
  overlay: { position: "fixed" as const, inset: 0, background: "#00000090", backdropFilter: "blur(4px)",
    display: "flex", alignItems: "center", justifyContent: "center", zIndex: 200 },
  modalBox: { background: C.surface, border: `1px solid ${C.border}`, borderRadius: 12, padding: "20px",
    width: 420, maxWidth: "90vw", maxHeight: "80vh", overflowY: "auto", boxShadow: "0 20px 60px rgba(0,0,0,0.5)" },

  // ─── UI-1 additions ─────────────────────────────────────────────────────
  // Per-message action toolbar — always present in DOM, fades in on hover.
  actionBar: { display: "inline-flex", gap: 4, alignItems: "center", marginLeft: "auto",
    transition: "opacity 0.15s ease" },
  actionBtn: { background: "transparent", border: "none", color: C.muted, fontSize: 13,
    cursor: "pointer", padding: "2px 6px", borderRadius: 4,
    transition: "background 0.12s, color 0.12s" } as React.CSSProperties,

  // Inline approval prompt for `RequireApproval` tools.
  approvalCard: { background: "#3a2a14", border: "1px solid #b45309",
    borderRadius: 8, padding: "10px 12px" },
  approvalBadge: { background: "#fbbf24", color: "#1e1500", fontSize: 10, fontWeight: 700,
    padding: "2px 6px", borderRadius: 3, letterSpacing: 0.4 },
  approvalArgs: { background: "#1c140844", borderRadius: 5, padding: "6px 8px",
    fontSize: 11.5, fontFamily: MONO, color: "#e5d3a8", maxHeight: 180,
    overflow: "auto", margin: "6px 0 0", whiteSpace: "pre-wrap" as const, wordBreak: "break-word" as const },
  approveBtn: { background: "#16a34a", color: "#fff", border: "none", borderRadius: 6,
    padding: "6px 12px", fontSize: 12, fontWeight: 600, cursor: "pointer" },
  rejectBtn:  { background: "#dc2626", color: "#fff", border: "none", borderRadius: 6,
    padding: "6px 12px", fontSize: 12, fontWeight: 600, cursor: "pointer" },

  // Streaming cursor — pulsing block at the end of a streaming response.
  streamingCursor: { display: "inline-block", width: 8, height: 16, background: C.accent,
    marginLeft: 3, borderRadius: 1, verticalAlign: "text-bottom" as const,
    animation: "owl-cursor-blink 1s steps(2) infinite" } as React.CSSProperties,
  streamingCursorInline: { display: "inline-block", width: 8, height: 16, background: C.accent,
    marginLeft: 3, borderRadius: 1, verticalAlign: "text-bottom" as const,
    animation: "owl-cursor-blink 1s steps(2) infinite" } as React.CSSProperties,

  // Composer attachment strip + drop overlay + Stop button.
  attachmentStrip: { display: "flex", flexWrap: "wrap" as const, gap: 6, marginBottom: 6,
    padding: "0 4px" },
  attachmentChip: { display: "inline-flex", alignItems: "center", gap: 6,
    background: C.surface, border: `1px solid ${C.border}`, borderRadius: 8,
    padding: "4px 6px 4px 4px" },
  chipClose: { background: "transparent", border: "none", color: C.muted, cursor: "pointer",
    fontSize: 16, lineHeight: "12px", padding: "0 4px" },
  attachBtn: { background: "transparent", border: "none", color: C.muted, fontSize: 16,
    cursor: "pointer", padding: "6px 8px", alignSelf: "center" } as React.CSSProperties,
  dropOverlay: { position: "absolute" as const, inset: 0, background: `${C.bg}dd`,
    border: `2px dashed ${C.accent}`, borderRadius: 12,
    display: "flex", alignItems: "center", justifyContent: "center", zIndex: 5,
    pointerEvents: "none" as const },
  stopBigBtn: { position: "absolute" as const, top: -42, left: "50%", transform: "translateX(-50%)",
    background: "#1e1e2a", color: "#f87171", border: "1px solid #6a2020", borderRadius: 16,
    padding: "5px 14px", fontSize: 12, fontWeight: 600, cursor: "pointer",
    display: "inline-flex", alignItems: "center", gap: 6, boxShadow: "0 4px 12px rgba(0,0,0,0.4)" },

  // Compact file attachment chip used inside user message bubbles.
  fileChip: { display: "inline-flex", alignItems: "center", gap: 6,
    background: C.surface, border: `1px solid ${C.border}`, borderRadius: 6,
    padding: "4px 8px", color: C.text },
};

// ─── Global CSS ───────────────────────────────────────────────────────────────
const _style = document.createElement("style");
_style.textContent = `
  @keyframes pulse  { 0%,80%,100%{opacity:.2;transform:scale(.85);} 40%{opacity:1;transform:scale(1);} }
  @keyframes shimmer { 0%{background-position:200% 0} 100%{background-position:-200% 0} }
  @keyframes spin    { from{transform:rotate(0deg)} to{transform:rotate(360deg)} }
  @keyframes avatarPulse {
    0%,100% { box-shadow: 0 0 0 0 rgba(204,120,92,0.55); }
    50%     { box-shadow: 0 0 0 4px rgba(204,120,92,0); }
  }
  @keyframes owl-cursor-blink { 0%,100% { opacity: 1; } 50% { opacity: 0.15; } }
  /* Owl chibi pet animations — layered CSS transforms; subtle for ambient,
     punchy for happy/excited.  Pet feels "alive" without depending on
     spritesheet frame swaps. */
  @keyframes owl-pet-idle    { 0%,100% { transform: translateY(0)   scale(1) } 50% { transform: translateY(-2px) scale(1) } }
  @keyframes owl-pet-bounce  { 0%,100% { transform: translateY(0)   scale(1) } 50% { transform: translateY(-6px) scale(1.06) } }
  @keyframes owl-pet-sleep   { 0%,100% { transform: scale(1) }       50% { transform: scale(0.95) } }
  @keyframes owl-pet-breathe { 0%,100% { transform: scale(1)   translateY(0) }    50% { transform: scale(1.03) translateY(-1px) } }
  @keyframes owl-pet-wiggle  {
    0%, 100% { transform: rotate(0deg) translateY(0) }
    25%      { transform: rotate(-3deg) translateY(-1px) }
    75%      { transform: rotate(3deg) translateY(-1px) }
  }
  @keyframes owl-pet-droop   { 0%,100% { transform: translateY(0) } 50% { transform: translateY(1px) } }
  @keyframes owl-pet-bubble  { from { opacity: 0; transform: translate(-50%, 6px) } to { opacity: 1; transform: translate(-50%, 0) } }
  @keyframes owl-pet-celebrate { 0% { opacity: 0; transform: translate(-50%, 10px) scale(0.8) } 30% { opacity: 1; transform: translate(-50%, 0) scale(1.1) } 80% { opacity: 1; transform: translate(-50%, -4px) scale(1) } 100% { opacity: 0; transform: translate(-50%, -12px) scale(0.9) } }
  /* Reveal pet's close button only on hover so it doesn't pollute the chrome. */
  [data-pet-root]:hover [data-pet-close] { opacity: 1 !important; }
  /* Action toolbar: visible at low opacity, brightens on parent hover. */
  [data-msg-row]:hover [data-action-bar] { opacity: 1 !important; }
  [data-action-btn]:hover { background: rgba(255,255,255,0.06); color: #fff !important; }
  ::-webkit-scrollbar { width: 4px; height: 4px; }
  ::-webkit-scrollbar-track { background: transparent; }
  ::-webkit-scrollbar-thumb { background: #2e2e3a; border-radius: 99px; }
  textarea::placeholder, input::placeholder { color: #40405a; }
  button:not(:disabled):hover { opacity: 0.85; }
  .hljs { background: transparent; color: #c8d3e8; }
  .hljs-comment,.hljs-quote { color: #5a6170; font-style: italic; }
  .hljs-keyword,.hljs-selector-tag,.hljs-built_in,.hljs-name,.hljs-tag { color: #c792ea; }
  .hljs-string,.hljs-doctag,.hljs-regexp { color: #c3e88d; }
  .hljs-number,.hljs-literal,.hljs-variable,.hljs-template-variable { color: #f78c6c; }
  .hljs-title,.hljs-section,.hljs-selector-id { color: #82aaff; font-weight: bold; }
  .hljs-type,.hljs-params,.hljs-class .hljs-title { color: #ffcb6b; }
  .hljs-attr,.hljs-attribute { color: #89ddf0; }
  .hljs-operator,.hljs-punctuation { color: #89ddf0; }
`;
document.head.appendChild(_style);
