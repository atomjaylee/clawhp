import { useState, useEffect, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  BarChart3,
  Loader2,
  RefreshCw,
  Zap,
  ArrowUpRight,
  ArrowDownLeft,
  Hash,
  Layers,
  Clock,
  TrendingUp,
  Crown,
  MessageSquare,
  Gauge,
  Database,
  AlertCircle,
  DollarSign,
  Wrench,
  Radio,
  Bot,
  Server,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import ModuleTabs, { type ModuleTabItem } from "@/components/ui/module-tabs";
import PageShell from "@/components/PageShell";
import { cn } from "@/lib/utils";
import type { CommandResult } from "@/types";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type UsageModuleTab = "overview" | "models" | "details";
type UsagePreset = "today" | "7d" | "30d" | "custom";

interface ModelEntry {
  name: string;
  provider?: string;
  tokens: number;
  inputTokens: number;
  outputTokens: number;
  messages: number;
  cost: number;
}

interface RankedEntry {
  name: string;
  tokens: number;
  cost: number;
  extra?: string;
}

interface SnapshotHealth {
  partial: boolean;
  indexedFiles: number;
  liveSessions: number;
  archivedFiles: number;
  providerUsageEnriched: boolean;
  warnings: string[];
}

interface UsageSnapshot {
  source: "gateway_api" | "status" | "local_logs" | "empty";
  apiPath?: string;
  health?: SnapshotHealth;
  // core metrics
  messages: number;
  userMessages: number;
  assistantMessages: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number;
  promptTokens: number;
  throughputPerMin: number;
  avgTokensPerMsg: number;
  totalCost: number;
  avgCostPerMsg: number;
  cacheHitRate: number;
  errorRate: number;
  sessionCount: number;
  sessionsInRange: number;
  avgSessionDuration: number;
  errorCount: number;
  toolCallCount: number;
  toolsUsed: number;
  // breakdowns
  models: ModelEntry[];
  providers: RankedEntry[];
  channels: RankedEntry[];
  tools: Array<{ name: string; calls: number }>;
  agents: RankedEntry[];
}

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

const TC = {
  input: { dot: "bg-blue-500", text: "text-blue-400", bar: "bg-blue-500/80", stroke: "#3b82f6" },
  output: { dot: "bg-violet-500", text: "text-violet-400", bar: "bg-violet-500/80", stroke: "#8b5cf6" },
};

const RANK_STYLES = [
  "from-amber-500/20 to-amber-500/5 border-amber-500/20",
  "from-slate-400/15 to-slate-400/5 border-slate-400/15",
  "from-orange-600/15 to-orange-600/5 border-orange-600/15",
];

// ---------------------------------------------------------------------------
// Flexible parser: handles various response shapes from gateway API / CLI
// ---------------------------------------------------------------------------

function nv(v: unknown): number { return Number(v) || 0; }
function ns(v: unknown): string { return String(v ?? ""); }

function pickNum(obj: Record<string, unknown>, ...keys: string[]): number {
  for (const k of keys) {
    const v = obj[k];
    if (v !== undefined && v !== null) return nv(v);
  }
  return 0;
}

function pickArr(obj: Record<string, unknown>, ...keys: string[]): unknown[] {
  for (const k of keys) {
    const v = obj[k];
    if (Array.isArray(v)) return v;
  }
  return [];
}

function pickObj(obj: Record<string, unknown>, ...keys: string[]): Record<string, unknown> {
  for (const k of keys) {
    const v = obj[k];
    if (v && typeof v === "object" && !Array.isArray(v)) return v as Record<string, unknown>;
  }
  return {};
}

function parseRanked(arr: unknown[]): RankedEntry[] {
  return arr.map((item) => {
    const r = item as Record<string, unknown>;
    return {
      name: ns(r.name ?? r.model ?? r.id ?? r.channel ?? r.provider ?? r.agent ?? "unknown"),
      tokens: pickNum(r, "tokens", "total_tokens", "totalTokens", "token_count"),
      cost: pickNum(r, "cost", "total_cost", "totalCost"),
      extra: r.messages !== undefined ? `${nv(r.messages)} msgs` : undefined,
    };
  });
}

function parseModels(arr: unknown[]): ModelEntry[] {
  return arr.map((item) => {
    const r = item as Record<string, unknown>;
    const inp = pickNum(r, "input_tokens", "inputTokens", "prompt_tokens", "promptTokens");
    const out = pickNum(r, "output_tokens", "outputTokens", "completion_tokens", "completionTokens");
    const total = pickNum(r, "tokens", "total_tokens", "totalTokens", "token_count") || (inp + out);
    return {
      name: ns(r.name ?? r.model ?? r.id ?? "unknown"),
      provider: r.provider ? ns(r.provider) : undefined,
      tokens: total,
      inputTokens: inp || Math.round(total * 0.8),
      outputTokens: out || total - Math.round(total * 0.8),
      messages: pickNum(r, "messages", "message_count", "requests", "count", "msgs"),
      cost: pickNum(r, "cost", "total_cost", "totalCost"),
    };
  });
}

function parseTools(arr: unknown[]): Array<{ name: string; calls: number }> {
  return arr.map((item) => {
    const r = item as Record<string, unknown>;
    return {
      name: ns(r.name ?? r.tool ?? r.id ?? "unknown"),
      calls: pickNum(r, "calls", "count", "invocations", "call_count"),
    };
  });
}

function emptySnapshot(source: UsageSnapshot["source"] = "empty"): UsageSnapshot {
  return {
    source, messages: 0, userMessages: 0, assistantMessages: 0,
    totalTokens: 0, inputTokens: 0, outputTokens: 0, cachedTokens: 0, promptTokens: 0,
    throughputPerMin: 0, avgTokensPerMsg: 0, totalCost: 0, avgCostPerMsg: 0,
    cacheHitRate: 0, errorRate: 0, sessionCount: 0, sessionsInRange: 0,
    avgSessionDuration: 0, errorCount: 0, toolCallCount: 0, toolsUsed: 0,
    models: [], providers: [], channels: [], tools: [], agents: [],
  };
}

function parseHealth(value: unknown): SnapshotHealth | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const health = value as Record<string, unknown>;
  return {
    partial: Boolean(health.partial),
    indexedFiles: pickNum(health, "indexedFiles", "indexed_files"),
    liveSessions: pickNum(health, "liveSessions", "live_sessions"),
    archivedFiles: pickNum(health, "archivedFiles", "archived_files"),
    providerUsageEnriched: Boolean(health.providerUsageEnriched ?? health.provider_usage_enriched),
    warnings: Array.isArray(health.warnings) ? health.warnings.map((item) => ns(item)).filter(Boolean) : [],
  };
}

function parseGatewayApi(data: Record<string, unknown>, path: string): UsageSnapshot {
  const msgObj = pickObj(data, "messages", "message_stats");
  const tokObj = pickObj(data, "tokens", "token_stats", "token_usage");
  const sessObj = pickObj(data, "sessions", "session_stats");
  const cacheObj = pickObj(data, "cache", "cache_stats");
  const errObj = pickObj(data, "errors", "error_stats");
  const costObj = pickObj(data, "cost", "billing");
  const tpObj = pickObj(data, "throughput");
  const tcObj = pickObj(data, "tool_calls", "toolCalls", "tool_call_stats");

  const totalMessages = pickNum(msgObj, "total", "count") || pickNum(data, "messages", "message_count", "totalMessages", "total_messages");
  const userMsgs = pickNum(msgObj, "user", "user_count") || pickNum(data, "user_messages");
  const asstMsgs = pickNum(msgObj, "assistant", "assistant_count") || pickNum(data, "assistant_messages");

  const totalTokens = pickNum(tokObj, "total", "count") || pickNum(data, "total_tokens", "totalTokens", "token_count");
  const inputTokens = pickNum(tokObj, "input", "input_tokens", "prompt") || pickNum(data, "input_tokens", "inputTokens", "prompt_tokens");
  const outputTokens = pickNum(tokObj, "output", "output_tokens", "completion") || pickNum(data, "output_tokens", "outputTokens", "completion_tokens");
  const cachedTokens = pickNum(cacheObj, "cached", "cached_tokens") || pickNum(tokObj, "cached") || pickNum(data, "cached_tokens");
  const promptTokens = pickNum(cacheObj, "prompt", "prompt_tokens") || pickNum(tokObj, "prompt") || pickNum(data, "prompt_tokens");

  const throughput = pickNum(tpObj, "tokens_per_min", "tok_per_min") || pickNum(data, "throughput", "tokens_per_min", "tokensPerMin", "tok_per_min");
  const avgTpMsg = pickNum(data, "avg_tokens_per_msg", "avgTokensPerMsg") || (totalMessages > 0 ? Math.round(totalTokens / totalMessages) : 0);

  const cost = pickNum(costObj, "total", "total_cost") || pickNum(data, "total_cost", "totalCost", "cost");
  const avgCost = pickNum(costObj, "avg_per_msg", "avg_cost_per_msg") || pickNum(data, "avg_cost_per_msg", "avgCostPerMsg") || (totalMessages > 0 ? cost / totalMessages : 0);

  const hitRate = pickNum(cacheObj, "hit_rate", "hitRate") || pickNum(data, "cache_hit_rate", "cacheHitRate");
  const errRate = pickNum(errObj, "rate", "error_rate") || pickNum(data, "error_rate", "errorRate");
  const sessions = pickNum(sessObj, "total", "count") || pickNum(data, "session_count", "sessionCount", "sessions");
  const inRange = pickNum(sessObj, "in_range", "inRange") || sessions;
  const avgDur = pickNum(sessObj, "avg_duration", "avgDuration") || pickNum(data, "avg_session_duration");
  const errors = pickNum(errObj, "total", "count") || pickNum(data, "errors", "error_count", "errorCount");
  const toolCalls = pickNum(tcObj, "total", "count") || pickNum(data, "tool_calls", "toolCalls", "tool_call_count");
  const toolsUsedN = pickNum(tcObj, "tools_used", "toolsUsed") || pickNum(data, "tools_used", "toolsUsed");

  const modelsArr = pickArr(data, "models", "top_models", "topModels", "model_usage");
  const providersArr = pickArr(data, "providers", "top_providers", "topProviders");
  const channelsArr = pickArr(data, "channels", "top_channels", "topChannels");
  const toolsArr = pickArr(data, "tools", "top_tools", "topTools");
  const agentsArr = pickArr(data, "agents", "top_agents", "topAgents");

  return {
    source: "gateway_api", apiPath: path,
    messages: totalMessages, userMessages: userMsgs, assistantMessages: asstMsgs,
    totalTokens: totalTokens || (inputTokens + outputTokens),
    inputTokens, outputTokens, cachedTokens, promptTokens,
    throughputPerMin: throughput, avgTokensPerMsg: avgTpMsg,
    totalCost: cost, avgCostPerMsg: avgCost,
    cacheHitRate: hitRate, errorRate: errRate,
    sessionCount: sessions, sessionsInRange: inRange, avgSessionDuration: avgDur,
    errorCount: errors, toolCallCount: toolCalls, toolsUsed: toolsUsedN,
    models: parseModels(modelsArr),
    providers: parseRanked(providersArr),
    channels: parseRanked(channelsArr),
    tools: parseTools(toolsArr),
    agents: parseRanked(agentsArr),
  };
}

function parseStatusFallback(status: Record<string, unknown>): UsageSnapshot {
  const snap = emptySnapshot("status");
  const sess = pickObj(status, "sessions");
  const recent = pickArr(sess, "recent");
  const defModel = ns((pickObj(sess, "defaults")).model ?? "unknown");

  const modelMap = new Map<string, ModelEntry>();
  for (const item of recent) {
    const s = item as Record<string, unknown>;
    const model = ns(s.model ?? defModel);
    const inp = pickNum(s, "inputTokens", "input_tokens", "promptTokens", "prompt_tokens");
    const out = pickNum(s, "outputTokens", "output_tokens", "completionTokens", "completion_tokens");
    const existing = modelMap.get(model) ?? { name: model, tokens: 0, inputTokens: 0, outputTokens: 0, messages: 0, cost: 0 };
    existing.tokens += inp + out;
    existing.inputTokens += inp;
    existing.outputTokens += out;
    existing.messages += 1;
    modelMap.set(model, existing);
  }

  snap.models = Array.from(modelMap.values());
  snap.messages = recent.length;
  snap.sessionCount = recent.length;
  snap.inputTokens = snap.models.reduce((s, m) => s + m.inputTokens, 0);
  snap.outputTokens = snap.models.reduce((s, m) => s + m.outputTokens, 0);
  snap.totalTokens = snap.inputTokens + snap.outputTokens;
  return snap;
}

function parseResponse(raw: string): UsageSnapshot {
  let json: Record<string, unknown>;
  try { json = JSON.parse(raw) as Record<string, unknown>; } catch { return emptySnapshot(); }

  const src = ns(json._source);
  if (src === "empty") return emptySnapshot();

  if (src === "gateway_api") {
    const data = (json.data ?? {}) as Record<string, unknown>;
    const path = ns(json._path);
    return parseGatewayApi(data, path);
  }

  if (src === "local_logs") {
    const parsed = parseGatewayApi(json, "local_logs");
    return { ...parsed, source: "local_logs", health: parseHealth(json.health) };
  }

  if (src === "status") {
    const status = (json.status ?? {}) as Record<string, unknown>;
    return parseStatusFallback(status);
  }

  return parseGatewayApi(json, "unknown");
}

// ---------------------------------------------------------------------------
// Formatters
// ---------------------------------------------------------------------------

function ft(v: number): string {
  if (v >= 1_000_000_000) return `${(v / 1e9).toFixed(1)}B`;
  if (v >= 1_000_000) return `${(v / 1e6).toFixed(1)}M`;
  if (v >= 1_000) return `${(v / 1e3).toFixed(1)}k`;
  return String(Math.round(v));
}

function fn(v: number): string { return v.toLocaleString("zh-CN"); }
function fPct(v: number): string { return `${(v * 100).toFixed(1)}%`; }
function fCost(v: number): string { return `$${v.toFixed(v >= 1 ? 2 : 4)}`; }

function formatDateInput(date: Date): string {
  const year = date.getFullYear();
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const day = `${date.getDate()}`.padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function resolvePresetRange(preset: Exclude<UsagePreset, "custom">): { startDate: string; endDate: string } {
  const end = new Date();
  end.setHours(0, 0, 0, 0);
  const start = new Date(end);
  const daysBack = preset === "today" ? 0 : preset === "7d" ? 6 : 29;
  start.setDate(start.getDate() - daysBack);
  return {
    startDate: formatDateInput(start),
    endDate: formatDateInput(end),
  };
}

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

export default function UsagePage() {
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<UsageSnapshot>(() => emptySnapshot());
  const [tab, setTab] = useState<UsageModuleTab>("overview");
  const [preset, setPreset] = useState<UsagePreset>("7d");
  const [startDate, setStartDate] = useState(() => resolvePresetRange("7d").startDate);
  const [endDate, setEndDate] = useState(() => resolvePresetRange("7d").endDate);

  const fetchUsage = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result: CommandResult = await invoke("get_usage_snapshot", { startDate, endDate });
      if (result.success) {
        setData(parseResponse(result.stdout));
      } else {
        setError(result.stderr || "获取用量数据失败");
      }
    } catch (e) {
      setError(`${e}`);
    } finally {
      setLoading(false);
    }
  }, [endDate, startDate]);

  useEffect(() => { void fetchUsage(); }, [fetchUsage]);

  const hasData = data.totalTokens > 0 || data.messages > 0 || data.models.length > 0;
  const isRefreshing = loading && hasData;
  const hasBreakdowns = data.providers.length > 0 || data.channels.length > 0 || data.tools.length > 0 || data.agents.length > 0;
  const rangeLabel = useMemo(
    () => `${startDate}${startDate !== endDate ? ` 至 ${endDate}` : ""}`,
    [endDate, startDate],
  );
  const subtitle = data.source === "local_logs"
    ? `基于本地会话按 OpenClaw 用量口径聚合${data.health?.partial ? "（部分明细已降级）" : ""}`
    : "跟踪 Token 消耗和 API 请求，了解各模型的资源使用分布";

  const moduleTabs: ModuleTabItem<UsageModuleTab>[] = [
    { id: "overview", label: "概览", icon: BarChart3 },
    { id: "models", label: "模型明细", icon: Layers, badge: data.models.length || undefined },
    ...(hasBreakdowns ? [{ id: "details" as const, label: "详情", icon: Server, badge: (data.providers.length + data.channels.length + data.tools.length) || undefined }] : []),
  ];

  const applyPreset = useCallback((nextPreset: Exclude<UsagePreset, "custom">) => {
    const range = resolvePresetRange(nextPreset);
    setPreset(nextPreset);
    setStartDate(range.startDate);
    setEndDate(range.endDate);
  }, []);

  const updateStartDate = useCallback((nextValue: string) => {
    setPreset("custom");
    setStartDate(nextValue);
    setEndDate((current) => (current < nextValue ? nextValue : current));
  }, []);

  const updateEndDate = useCallback((nextValue: string) => {
    setPreset("custom");
    setEndDate(nextValue);
    setStartDate((current) => (current > nextValue ? nextValue : current));
  }, []);

  return (
    <PageShell
      bodyClassName="space-y-4"
      header={
        <div className="flex items-center justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold">用量统计</h2>
            <p className="text-[11px] text-muted-foreground">{subtitle}</p>
          </div>
          <div className="flex items-center gap-2">
            {data.source !== "empty" && (
              <Badge variant="outline" className="border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">
                {data.source === "gateway_api"
                  ? "网关 API"
                  : data.source === "local_logs"
                    ? "本地口径"
                    : "CLI 快照"}
              </Badge>
            )}
            <Button size="sm" variant="outline" onClick={() => void fetchUsage()} disabled={loading}>
              {loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
              {loading ? "同步中" : "刷新"}
            </Button>
          </div>
        </div>
      }
    >
      {error && (
        <Card className="border-red-500/20">
          <CardContent className="p-3">
            <p className="text-[12px] text-red-400">{error}</p>
          </CardContent>
        </Card>
      )}

      <div className="relative">
        {isRefreshing && <RefreshOverlay rangeLabel={rangeLabel} />}

        <div className={cn("space-y-4 transition-opacity duration-200", isRefreshing && "pointer-events-none select-none")}>
          <Card>
            <CardContent className="flex flex-col gap-3 p-3 lg:flex-row lg:items-center lg:justify-between">
              <div className="flex flex-wrap items-center gap-2">
                {([
                  { id: "today", label: "今天" },
                  { id: "7d", label: "7 天" },
                  { id: "30d", label: "30 天" },
                ] as const).map((item) => (
                  <Button
                    key={item.id}
                    size="sm"
                    variant={preset === item.id ? "default" : "outline"}
                    className={cn("h-8", preset === item.id && "shadow-none")}
                    onClick={() => applyPreset(item.id)}
                    disabled={loading}
                  >
                    {item.label}
                  </Button>
                ))}

                <div className="flex items-center gap-2 rounded-lg border border-white/[0.06] bg-white/[0.02] px-2.5 py-1.5">
                  <input
                    type="date"
                    value={startDate}
                    onChange={(event) => updateStartDate(event.target.value)}
                    disabled={loading}
                    className="bg-transparent text-[11px] text-foreground/85 outline-none disabled:cursor-not-allowed disabled:opacity-60"
                  />
                  <span className="text-[10px] text-muted-foreground">至</span>
                  <input
                    type="date"
                    value={endDate}
                    onChange={(event) => updateEndDate(event.target.value)}
                    disabled={loading}
                    className="bg-transparent text-[11px] text-foreground/85 outline-none disabled:cursor-not-allowed disabled:opacity-60"
                  />
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-end gap-2 text-[11px] text-muted-foreground">
                {isRefreshing && (
                  <span className="inline-flex items-center gap-1.5 rounded-full border border-cyan-500/20 bg-cyan-500/10 px-2.5 py-1 text-cyan-300">
                    <Loader2 size={12} className="animate-spin" />
                    正在切换时间范围
                  </span>
                )}
                <span>当前范围：{rangeLabel}</span>
              </div>
            </CardContent>
          </Card>

          <MetricsGrid data={data} loading={loading && !hasData} />

          <ModuleTabs items={moduleTabs} value={tab} onValueChange={setTab} />

          {tab === "overview" && (hasData ? <OverviewContent data={data} /> : <EmptyState loading={loading} />)}
          {tab === "models" && (hasData ? <ModelDetailContent models={data.models} totalTokens={data.totalTokens} /> : <EmptyState loading={loading} />)}
          {tab === "details" && <DetailsContent data={data} />}
        </div>
      </div>
    </PageShell>
  );
}

// ---------------------------------------------------------------------------
// Metrics grid – matches OpenClaw web panel layout
// ---------------------------------------------------------------------------

function MetricsGrid({ data, loading }: { data: UsageSnapshot; loading: boolean }) {
  const total = data.inputTokens + data.outputTokens;
  const inputPct = total > 0 ? (data.inputTokens / total) * 100 : 50;

  return (
    <Card className="overflow-hidden">
      <CardContent className="p-0">
        <div className="flex flex-col gap-4 p-4 lg:flex-row lg:items-center">
          <div className="flex items-center gap-4 shrink-0">
            <div className="relative">
              {loading ? (
                <div className="h-[88px] w-[88px] animate-pulse rounded-full bg-white/[0.04]" />
              ) : (
                <>
                  <TokenRing inputPct={inputPct} size={88} />
                  <div className="absolute inset-0 flex flex-col items-center justify-center">
                    <span className="text-[15px] font-bold tabular-nums text-foreground/90">{ft(data.totalTokens || total)}</span>
                    <span className="text-[8px] uppercase tracking-wider text-muted-foreground">TOKENS</span>
                  </div>
                </>
              )}
            </div>
            <div className="space-y-1 lg:hidden"><RingLegend /></div>
          </div>

          <div className="grid flex-1 grid-cols-2 gap-2 lg:grid-cols-4">
            <StatBlock label="消息数" value={fn(data.messages)} icon={MessageSquare} color="text-foreground/80" loading={loading}
              sub={data.userMessages > 0 ? `${data.userMessages} 用户 · ${data.assistantMessages} 助手` : undefined} />
            <StatBlock label="吞吐量" value={data.throughputPerMin > 0 ? `${ft(data.throughputPerMin)}` : "—"} icon={Gauge} color="text-cyan-400" loading={loading}
              sub={data.throughputPerMin > 0 ? "tok/min" : undefined} />
            <StatBlock label="输入 Token" value={ft(data.inputTokens)} icon={ArrowDownLeft} color={TC.input.text} loading={loading}
              sub={data.inputTokens >= 1000 ? fn(data.inputTokens) : undefined} />
            <StatBlock label="输出 Token" value={ft(data.outputTokens)} icon={ArrowUpRight} color={TC.output.text} loading={loading}
              sub={data.outputTokens >= 1000 ? fn(data.outputTokens) : undefined} />
          </div>

          <div className="hidden shrink-0 lg:block"><RingLegend /></div>
        </div>

        {(data.cacheHitRate > 0 || data.sessionCount > 0 || data.toolCallCount > 0 || data.totalCost > 0) && (
          <div className="border-t border-white/[0.06] px-4 py-3">
            <div className="grid grid-cols-2 gap-2 lg:grid-cols-5">
              {data.cacheHitRate > 0 && (
                <MiniStat label="缓存命中率" value={fPct(data.cacheHitRate)} icon={Database} color="text-emerald-400"
                  sub={data.cachedTokens > 0 ? `${ft(data.cachedTokens)} cached · ${ft(data.promptTokens)} prompt` : undefined} />
              )}
              {data.errorRate >= 0 && data.source !== "status" && data.source !== "empty" && (
                <MiniStat label="错误率" value={fPct(data.errorRate)} icon={AlertCircle}
                  color={data.errorRate > 0 ? "text-red-400" : "text-emerald-400"} />
              )}
              {data.sessionCount > 0 && (
                <MiniStat label="会话" value={fn(data.sessionCount)} icon={Hash} color="text-foreground/60"
                  sub={data.avgSessionDuration > 0 ? `${data.avgSessionDuration}s avg` : undefined} />
              )}
              {data.toolCallCount > 0 && (
                <MiniStat label="工具调用" value={fn(data.toolCallCount)} icon={Wrench} color="text-amber-400"
                  sub={data.toolsUsed > 0 ? `${data.toolsUsed} 种工具` : undefined} />
              )}
              {data.totalCost > 0 && (
                <MiniStat label="费用" value={fCost(data.totalCost)} icon={DollarSign} color="text-green-400"
                  sub={data.avgCostPerMsg > 0 ? `avg ${fCost(data.avgCostPerMsg)}/msg` : undefined} />
              )}
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function RefreshOverlay({ rangeLabel }: { rangeLabel: string }) {
  return (
    <div className="absolute inset-0 z-20 flex items-start justify-center rounded-[28px] bg-slate-950/58 backdrop-blur-[2px]">
      <div className="mt-14 flex min-w-[220px] items-center gap-3 rounded-2xl border border-cyan-400/20 bg-slate-950/90 px-4 py-3 shadow-[0_18px_60px_rgba(8,15,32,0.45)]">
        <div className="relative flex h-10 w-10 shrink-0 items-center justify-center rounded-full border border-cyan-400/20 bg-cyan-400/10 text-cyan-300">
          <span className="absolute inset-0 rounded-full border border-cyan-300/25 animate-ping" />
          <Loader2 size={16} className="relative animate-spin" />
        </div>
        <div className="min-w-0">
          <div className="text-[12px] font-medium text-foreground/90">正在加载该时间范围的统计</div>
          <div className="mt-1 truncate text-[10px] text-muted-foreground">{rangeLabel}</div>
        </div>
      </div>
    </div>
  );
}

function StatBlock({ label, value, icon: Icon, color, loading, sub }: {
  label: string; value: string; icon: typeof Zap; color: string; loading: boolean; sub?: string;
}) {
  return (
    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-2.5">
      <div className="flex items-center justify-between">
        <span className="text-[9px] uppercase tracking-wider text-muted-foreground/60">{label}</span>
        <Icon size={12} className={`${color} opacity-40`} />
      </div>
      {loading ? (
        <div className="mt-1.5 h-5 w-14 animate-pulse rounded bg-white/[0.06]" />
      ) : (
        <>
          <div className={`mt-1 text-base font-bold tabular-nums leading-none ${color}`}>{value}</div>
          {sub && <div className="mt-0.5 text-[8px] tabular-nums text-muted-foreground/30">{sub}</div>}
        </>
      )}
    </div>
  );
}

function MiniStat({ label, value, icon: Icon, color, sub }: {
  label: string; value: string; icon: typeof Zap; color: string; sub?: string;
}) {
  return (
    <div className="flex items-center gap-2.5 rounded-lg border border-white/[0.04] bg-white/[0.02] px-3 py-2">
      <Icon size={13} className={`${color} opacity-50 shrink-0`} />
      <div className="min-w-0">
        <div className="flex items-baseline gap-1.5">
          <span className={`text-[13px] font-bold tabular-nums leading-none ${color}`}>{value}</span>
          <span className="text-[9px] text-muted-foreground/50">{label}</span>
        </div>
        {sub && <div className="mt-0.5 text-[8px] tabular-nums text-muted-foreground/30 truncate">{sub}</div>}
      </div>
    </div>
  );
}

function RingLegend() {
  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-1.5"><div className={`h-2 w-2 rounded-full ${TC.input.dot}`} /><span className="text-[10px] text-muted-foreground">输入</span></div>
      <div className="flex items-center gap-1.5"><div className={`h-2 w-2 rounded-full ${TC.output.dot}`} /><span className="text-[10px] text-muted-foreground">输出</span></div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// SVG Ring chart
// ---------------------------------------------------------------------------

function TokenRing({ inputPct, size = 88 }: { inputPct: number; size?: number }) {
  const sw = 8;
  const r = (size - sw) / 2;
  const circ = 2 * Math.PI * r;
  const c = size / 2;
  const gap = 3;
  const inputLen = Math.max((inputPct / 100) * circ - gap, 0);
  const outputLen = Math.max(circ - inputLen - gap * 2, 0);

  return (
    <svg width={size} height={size} className="-rotate-90">
      <circle cx={c} cy={c} r={r} fill="none" stroke="rgba(255,255,255,0.04)" strokeWidth={sw} />
      <circle cx={c} cy={c} r={r} fill="none" stroke={TC.input.stroke} strokeWidth={sw}
        strokeDasharray={`${inputLen} ${circ - inputLen}`} strokeLinecap="round"
        className="transition-all duration-500" />
      <circle cx={c} cy={c} r={r} fill="none" stroke={TC.output.stroke} strokeWidth={sw}
        strokeDasharray={`${outputLen} ${circ - outputLen}`}
        strokeDashoffset={`${-(inputLen + gap)}`} strokeLinecap="round"
        className="transition-all duration-500" />
    </svg>
  );
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

function OverviewContent({ data }: { data: UsageSnapshot }) {
  const totalTokens = data.totalTokens || (data.inputTokens + data.outputTokens);

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-[1.3fr_0.7fr]">
      <Card>
        <CardContent className="p-5">
          <div className="mb-4 flex items-center justify-between">
            <div>
              <h3 className="text-[13px] font-semibold text-foreground/90">模型用量排行</h3>
              <p className="mt-0.5 text-[11px] text-muted-foreground">按总 Token 消耗降序排列</p>
            </div>
            {data.models.length > 0 && (
              <Badge variant="outline" className="border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">
                {data.models.length} 个模型
              </Badge>
            )}
          </div>
          {data.models.length > 0 ? (
            <ModelLeaderboard models={data.models} totalTokens={totalTokens} />
          ) : (
            <PlaceholderBox text="暂无模型级别的用量数据" />
          )}
        </CardContent>
      </Card>

      <div className="flex flex-col gap-4">
        <Card>
          <CardContent className="p-5">
            <h3 className="mb-3 text-[13px] font-semibold text-foreground/90">Token 构成</h3>
            {totalTokens > 0 ? (
              <TokenBreakdown inputTokens={data.inputTokens} outputTokens={data.outputTokens} />
            ) : (
              <PlaceholderBox text="暂无 Token 数据" />
            )}
          </CardContent>
        </Card>

        {data.channels.length > 0 && (
          <Card>
            <CardContent className="p-5">
              <div className="mb-3 flex items-center gap-2">
                <Radio size={14} className="text-muted-foreground/60" />
                <h3 className="text-[13px] font-semibold text-foreground/90">渠道分布</h3>
              </div>
              <RankedList items={data.channels} />
            </CardContent>
          </Card>
        )}

        {data.providers.length > 0 && (
          <Card>
            <CardContent className="p-5">
              <div className="mb-3 flex items-center gap-2">
                <Server size={14} className="text-muted-foreground/60" />
                <h3 className="text-[13px] font-semibold text-foreground/90">供应商</h3>
              </div>
              <RankedList items={data.providers} />
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Ranked list (for providers, channels, agents)
// ---------------------------------------------------------------------------

function RankedList({ items }: { items: RankedEntry[] }) {
  const sorted = useMemo(() => [...items].sort((a, b) => b.tokens - a.tokens), [items]);
  const maxTk = Math.max(...sorted.map((i) => i.tokens), 1);

  return (
    <div className="space-y-2">
      {sorted.map((item, idx) => {
        const barPct = (item.tokens / maxTk) * 100;
        return (
          <div key={item.name} className="space-y-1">
            <div className="flex items-center justify-between text-[11px]">
              <div className="flex items-center gap-2 min-w-0">
                <span className="shrink-0 w-4 text-right text-muted-foreground/40 tabular-nums">{idx + 1}</span>
                <span className="truncate font-medium text-foreground/80">{item.name}</span>
              </div>
              <div className="flex items-center gap-2 shrink-0 tabular-nums">
                {item.cost > 0 && <span className="text-green-400/60">{fCost(item.cost)}</span>}
                <span className="text-foreground/60">{ft(item.tokens)}</span>
                {item.extra && <span className="text-muted-foreground/40">{item.extra}</span>}
              </div>
            </div>
            <div className="ml-6 h-1.5 w-full overflow-hidden rounded-full bg-white/[0.04]">
              <div className="h-full rounded-full bg-primary/40 transition-all duration-500" style={{ width: `${Math.max(barPct, 3)}%` }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Model leaderboard
// ---------------------------------------------------------------------------

function ModelLeaderboard({ models, totalTokens }: { models: ModelEntry[]; totalTokens: number }) {
  const sorted = useMemo(
    () => [...models].sort((a, b) => b.tokens - a.tokens),
    [models],
  );
  const maxTk = Math.max(...sorted.map((m) => m.tokens), 1);

  return (
    <div className="space-y-2.5">
      {sorted.map((model, idx) => {
        const barPct = (model.tokens / maxTk) * 100;
        const inPct = model.tokens > 0 ? (model.inputTokens / model.tokens) * 100 : 0;
        const share = totalTokens > 0 ? (model.tokens / totalTokens) * 100 : 0;
        const top3 = idx < 3;

        return (
          <div key={model.name}
            className={cn("rounded-xl border p-3.5 transition-colors",
              top3 ? `bg-gradient-to-r ${RANK_STYLES[idx]}` : "border-white/[0.06] bg-white/[0.02] hover:bg-white/[0.03]",
            )}
          >
            <div className="mb-2 flex items-center justify-between gap-2">
              <div className="flex items-center gap-2.5 min-w-0">
                {top3 ? (
                  <div className="flex h-5 w-5 shrink-0 items-center justify-center rounded-md bg-white/[0.08] text-[10px] font-bold text-foreground/70">
                    {idx === 0 ? <Crown size={11} className="text-amber-400" /> : idx + 1}
                  </div>
                ) : (
                  <div className="flex h-5 w-5 shrink-0 items-center justify-center text-[10px] font-medium text-muted-foreground/50">{idx + 1}</div>
                )}
                <span className="truncate text-[12px] font-medium text-foreground/90">{model.name}</span>
                {model.provider && (
                  <Badge variant="outline" className="border-white/[0.08] bg-white/[0.03] text-[9px] text-muted-foreground">{model.provider}</Badge>
                )}
              </div>
              <div className="flex items-center gap-2 shrink-0">
                {model.cost > 0 && <span className="text-[10px] tabular-nums text-green-400/60">{fCost(model.cost)}</span>}
                <span className="text-[11px] font-semibold tabular-nums text-foreground/80">{ft(model.tokens)}</span>
                <span className="w-10 text-right text-[10px] tabular-nums text-muted-foreground/50">{share.toFixed(1)}%</span>
              </div>
            </div>

            <div className="h-2 w-full overflow-hidden rounded-full bg-white/[0.04]">
              <div className="flex h-full rounded-full transition-all duration-500 ease-out" style={{ width: `${Math.max(barPct, 3)}%` }}>
                <div className={`h-full ${TC.input.bar}`} style={{ width: `${inPct}%`, borderRadius: inPct >= 99 ? "9999px" : "9999px 0 0 9999px" }} />
                <div className={`h-full ${TC.output.bar}`} style={{ width: `${100 - inPct}%`, borderRadius: inPct <= 1 ? "9999px" : "0 9999px 9999px 0" }} />
              </div>
            </div>

            <div className="mt-1.5 flex items-center gap-3 text-[10px] tabular-nums text-muted-foreground/50">
              <span>{model.messages} 次请求</span>
              <span>·</span>
              <span className={TC.input.text}>入 {ft(model.inputTokens)}</span>
              <span className={TC.output.text}>出 {ft(model.outputTokens)}</span>
            </div>
          </div>
        );
      })}

      <div className="flex items-center gap-4 pt-1">
        <div className="flex items-center gap-1.5"><div className={`h-2 w-2 rounded-full ${TC.input.dot}`} /><span className="text-[10px] text-muted-foreground">输入</span></div>
        <div className="flex items-center gap-1.5"><div className={`h-2 w-2 rounded-full ${TC.output.dot}`} /><span className="text-[10px] text-muted-foreground">输出</span></div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Token breakdown
// ---------------------------------------------------------------------------

function TokenBreakdown({ inputTokens, outputTokens }: { inputTokens: number; outputTokens: number }) {
  const total = inputTokens + outputTokens;
  const inPct = (inputTokens / total) * 100;

  return (
    <div className="space-y-3.5">
      <div className="h-3.5 w-full overflow-hidden rounded-full bg-white/[0.04]">
        <div className="flex h-full">
          <div className={`h-full ${TC.input.bar} transition-all duration-700`} style={{ width: `${inPct}%` }} />
          <div className={`h-full ${TC.output.bar} transition-all duration-700`} style={{ width: `${100 - inPct}%` }} />
        </div>
      </div>
      <div className="grid grid-cols-2 gap-2.5">
        <BkStat color={TC.input.dot} cText={TC.input.text} label="输入" value={ft(inputTokens)} pct={inPct} raw={fn(inputTokens)} />
        <BkStat color={TC.output.dot} cText={TC.output.text} label="输出" value={ft(outputTokens)} pct={100 - inPct} raw={fn(outputTokens)} />
      </div>
    </div>
  );
}

function BkStat({ color, cText, label, value, pct, raw }: {
  color: string; cText: string; label: string; value: string; pct: number; raw: string;
}) {
  return (
    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
      <div className="flex items-center gap-2">
        <div className={`h-2 w-2 rounded-full ${color}`} />
        <span className="text-[10px] uppercase tracking-wider text-muted-foreground/60">{label}</span>
        <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/40">{pct.toFixed(1)}%</span>
      </div>
      <div className={`mt-1.5 text-[17px] font-bold tabular-nums leading-none ${cText}`}>{value}</div>
      <div className="mt-1 text-[9px] tabular-nums text-muted-foreground/30">{raw}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Model detail table
// ---------------------------------------------------------------------------

function ModelDetailContent({ models, totalTokens }: { models: ModelEntry[]; totalTokens: number }) {
  const sorted = useMemo(
    () => [...models].sort((a, b) => b.tokens - a.tokens),
    [models],
  );

  return (
    <Card>
      <CardContent className="p-5">
        <div className="mb-4">
          <h3 className="text-[13px] font-semibold text-foreground/90">模型明细</h3>
          <p className="mt-0.5 text-[11px] text-muted-foreground">各模型的请求次数和 Token 消耗</p>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full min-w-[560px]">
            <thead>
              <tr className="border-b border-white/[0.08]">
                <th className="w-8 pb-2.5 text-center text-[10px] font-medium text-muted-foreground/50">#</th>
                <th className="pb-2.5 pr-4 text-left text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">模型</th>
                <th className="pb-2.5 px-3 text-right text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">请求</th>
                <th className="pb-2.5 px-3 text-right text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  <span className="inline-flex items-center gap-1"><span className={`h-1.5 w-1.5 rounded-full ${TC.input.dot}`} />输入</span>
                </th>
                <th className="pb-2.5 px-3 text-right text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  <span className="inline-flex items-center gap-1"><span className={`h-1.5 w-1.5 rounded-full ${TC.output.dot}`} />输出</span>
                </th>
                <th className="pb-2.5 px-3 text-right text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">合计</th>
                <th className="pb-2.5 pl-3 text-right text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">占比</th>
              </tr>
            </thead>
            <tbody>
              {sorted.map((m, idx) => {
                const share = totalTokens > 0 ? (m.tokens / totalTokens) * 100 : 0;
                return (
                  <tr key={m.name} className="group border-b border-white/[0.04] last:border-0 transition-colors hover:bg-white/[0.02]">
                    <td className="py-3 text-center text-[11px] tabular-nums text-muted-foreground/40">{idx + 1}</td>
                    <td className="py-3 pr-4">
                      <div className="flex items-center gap-2">
                        <span className="text-[12px] font-medium text-foreground/85">{m.name}</span>
                        {m.provider && <Badge variant="outline" className="border-white/[0.08] bg-white/[0.03] text-[9px] text-muted-foreground">{m.provider}</Badge>}
                      </div>
                    </td>
                    <td className="py-3 px-3 text-right text-[12px] tabular-nums text-muted-foreground">{fn(m.messages)}</td>
                    <td className={`py-3 px-3 text-right text-[12px] font-medium tabular-nums ${TC.input.text}`}>{ft(m.inputTokens)}</td>
                    <td className={`py-3 px-3 text-right text-[12px] font-medium tabular-nums ${TC.output.text}`}>{ft(m.outputTokens)}</td>
                    <td className="py-3 px-3 text-right text-[12px] font-semibold tabular-nums text-foreground/80">{ft(m.tokens)}</td>
                    <td className="py-3 pl-3">
                      <div className="flex items-center justify-end gap-2">
                        <div className="h-1.5 w-16 overflow-hidden rounded-full bg-white/[0.04]">
                          <div className="h-full rounded-full bg-emerald-500/60 transition-all duration-500" style={{ width: `${Math.max(share, 2)}%` }} />
                        </div>
                        <span className="w-10 text-right text-[10px] tabular-nums text-muted-foreground/50">{share.toFixed(1)}%</span>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
            <tfoot>
              <tr className="border-t border-white/[0.08]">
                <td className="pt-3" />
                <td className="pt-3 pr-4 text-[12px] font-semibold text-foreground/70">合计</td>
                <td className="pt-3 px-3 text-right text-[12px] font-semibold tabular-nums text-foreground/70">{fn(sorted.reduce((s, m) => s + m.messages, 0))}</td>
                <td className={`pt-3 px-3 text-right text-[12px] font-bold tabular-nums ${TC.input.text}`}>{ft(sorted.reduce((s, m) => s + m.inputTokens, 0))}</td>
                <td className={`pt-3 px-3 text-right text-[12px] font-bold tabular-nums ${TC.output.text}`}>{ft(sorted.reduce((s, m) => s + m.outputTokens, 0))}</td>
                <td className="pt-3 px-3 text-right text-[12px] font-bold tabular-nums text-foreground/90">{ft(totalTokens)}</td>
                <td className="pt-3 pl-3 text-right text-[10px] tabular-nums text-muted-foreground/50">100%</td>
              </tr>
            </tfoot>
          </table>
        </div>
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Details tab – providers, channels, tools, agents
// ---------------------------------------------------------------------------

function DetailsContent({ data }: { data: UsageSnapshot }) {
  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      {data.providers.length > 0 && (
        <Card>
          <CardContent className="p-5">
            <div className="mb-3 flex items-center gap-2">
              <Server size={14} className="text-muted-foreground/60" />
              <h3 className="text-[13px] font-semibold text-foreground/90">供应商</h3>
              <Badge variant="outline" className="ml-auto border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">{data.providers.length}</Badge>
            </div>
            <RankedList items={data.providers} />
          </CardContent>
        </Card>
      )}

      {data.channels.length > 0 && (
        <Card>
          <CardContent className="p-5">
            <div className="mb-3 flex items-center gap-2">
              <Radio size={14} className="text-muted-foreground/60" />
              <h3 className="text-[13px] font-semibold text-foreground/90">渠道</h3>
              <Badge variant="outline" className="ml-auto border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">{data.channels.length}</Badge>
            </div>
            <RankedList items={data.channels} />
          </CardContent>
        </Card>
      )}

      {data.tools.length > 0 && (
        <Card>
          <CardContent className="p-5">
            <div className="mb-3 flex items-center gap-2">
              <Wrench size={14} className="text-muted-foreground/60" />
              <h3 className="text-[13px] font-semibold text-foreground/90">工具调用</h3>
              <Badge variant="outline" className="ml-auto border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">{data.tools.length}</Badge>
            </div>
            <ToolsList tools={data.tools} />
          </CardContent>
        </Card>
      )}

      {data.agents.length > 0 && (
        <Card>
          <CardContent className="p-5">
            <div className="mb-3 flex items-center gap-2">
              <Bot size={14} className="text-muted-foreground/60" />
              <h3 className="text-[13px] font-semibold text-foreground/90">Agent</h3>
              <Badge variant="outline" className="ml-auto border-white/[0.08] bg-white/[0.03] text-[10px] text-muted-foreground">{data.agents.length}</Badge>
            </div>
            <RankedList items={data.agents} />
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function ToolsList({ tools }: { tools: Array<{ name: string; calls: number }> }) {
  const sorted = useMemo(() => [...tools].sort((a, b) => b.calls - a.calls), [tools]);
  const maxCalls = Math.max(...sorted.map((t) => t.calls), 1);

  return (
    <div className="space-y-2">
      {sorted.map((tool, idx) => (
        <div key={tool.name} className="space-y-1">
          <div className="flex items-center justify-between text-[11px]">
            <div className="flex items-center gap-2 min-w-0">
              <span className="shrink-0 w-4 text-right text-muted-foreground/40 tabular-nums">{idx + 1}</span>
              <span className="truncate font-medium text-foreground/80">{tool.name}</span>
            </div>
            <span className="shrink-0 tabular-nums text-foreground/60">{fn(tool.calls)} calls</span>
          </div>
          <div className="ml-6 h-1.5 w-full overflow-hidden rounded-full bg-white/[0.04]">
            <div className="h-full rounded-full bg-amber-500/40 transition-all duration-500" style={{ width: `${Math.max((tool.calls / maxCalls) * 100, 3)}%` }} />
          </div>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Empty / placeholder
// ---------------------------------------------------------------------------

function EmptyState({ loading }: { loading: boolean }) {
  if (loading) {
    return (
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[1.3fr_0.7fr]">
        <Card><CardContent className="p-5 space-y-3">
          {[1, 2, 3].map((i) => (
            <div key={i} className="animate-pulse rounded-xl border border-white/[0.04] bg-white/[0.02] p-4">
              <div className="mb-2 flex justify-between"><div className="h-3 w-28 rounded bg-white/[0.06]" /><div className="h-3 w-16 rounded bg-white/[0.06]" /></div>
              <div className="h-2 w-full rounded-full bg-white/[0.04]" />
            </div>
          ))}
        </CardContent></Card>
        <Card><CardContent className="p-5 space-y-4">
          <div className="h-3.5 w-full animate-pulse rounded-full bg-white/[0.04]" />
          <div className="grid grid-cols-2 gap-2.5">
            {[1, 2].map((i) => (<div key={i} className="animate-pulse rounded-xl border border-white/[0.04] bg-white/[0.02] p-3"><div className="h-2 w-12 rounded bg-white/[0.06]" /><div className="mt-2 h-5 w-14 rounded bg-white/[0.06]" /></div>))}
          </div>
        </CardContent></Card>
      </div>
    );
  }

  return (
    <Card>
      <CardContent className="flex flex-col items-center justify-center py-16 px-6">
        <div className="flex h-16 w-16 items-center justify-center rounded-2xl bg-gradient-to-br from-white/[0.06] to-white/[0.02]">
          <Clock size={28} className="text-muted-foreground/30" />
        </div>
        <h3 className="mt-5 text-[14px] font-semibold text-foreground/80">暂无用量数据</h3>
        <p className="mt-2 max-w-md text-center text-[12px] leading-relaxed text-muted-foreground">
          当会话产生回复后，用量会从本地日志或运行时快照自动汇总到这里。请确保 OpenClaw 已启动并存在活跃会话。
        </p>
      </CardContent>
    </Card>
  );
}

function PlaceholderBox({ text }: { text: string }) {
  return (
    <div className="rounded-xl border border-white/[0.06] bg-white/[0.02] p-8 text-center">
      <p className="text-[12px] text-muted-foreground/60">{text}</p>
    </div>
  );
}
