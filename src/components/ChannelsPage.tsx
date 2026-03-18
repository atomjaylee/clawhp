import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import QRCode from "qrcode";
import {
  Bot,
  ChevronDown,
  ChevronUp,
  CheckCircle2,
  Loader2,
  MessageSquare,
  Plus,
  Radio,
  RefreshCw,
  Route,
  Settings2,
  Trash2,
  Wifi,
  WifiOff,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import PageShell from "@/components/PageShell";
import type { AgentInfo, CommandResult, LogEntry } from "@/types";

interface ChannelEntry {
  channel: string;
  account: string;
  name: string;
  enabled: boolean;
}

interface ChannelStatus {
  channel: string;
  account: string;
  state: "online" | "configured" | "issue" | "disabled";
  message: string;
}

interface FeishuPluginStatus {
  officialPluginInstalled: boolean;
  officialPluginEnabled: boolean;
  communityPluginEnabled: boolean;
  channelConfigured: boolean;
  appId: string;
  displayName: string;
  domain: string;
}

interface FeishuInstallEvent {
  level: string;
  message: string;
}

interface FeishuAuthStartPayload {
  verificationUrl: string;
  deviceCode: string;
  intervalSeconds: number;
  expireInSeconds: number;
  env: string;
  domain: string;
}

interface FeishuAuthPollPayload {
  status: "pending" | "slow_down" | "success" | "denied" | "expired" | "error";
  suggestedDomain?: string | null;
  tenantBrand?: string | null;
  appId?: string | null;
  appSecret?: string | null;
  openId?: string | null;
  error?: string | null;
  errorDescription?: string | null;
}

type FeishuSetupStep = "install" | "bind" | "done";

interface ExistingFeishuBindingForm {
  appId: string;
  appSecret: string;
  domain: "feishu" | "lark";
}

type FeishuRouteScope = "account" | "direct" | "group";

interface FeishuMultiAgentRoute {
  agentId: string;
  scope: FeishuRouteScope;
  accountId?: string | null;
  peerId?: string | null;
}

interface FeishuMultiAgentBindingsSnapshot {
  defaultAccountId: string;
  routes: FeishuMultiAgentRoute[];
}

const CHANNEL_LABELS: Record<string, { label: string; color: string }> = {
  telegram: { label: "Telegram", color: "bg-sky-500/15 text-sky-400" },
  whatsapp: { label: "WhatsApp", color: "bg-emerald-500/15 text-emerald-400" },
  discord: { label: "Discord", color: "bg-indigo-500/15 text-indigo-400" },
  feishu: { label: "飞书", color: "bg-blue-500/15 text-blue-400" },
  slack: { label: "Slack", color: "bg-purple-500/15 text-purple-400" },
  signal: { label: "Signal", color: "bg-blue-400/15 text-blue-300" },
  imessage: { label: "iMessage", color: "bg-green-500/15 text-green-400" },
  googlechat: { label: "Google Chat", color: "bg-yellow-500/15 text-yellow-400" },
  matrix: { label: "Matrix", color: "bg-teal-500/15 text-teal-400" },
  msteams: { label: "MS Teams", color: "bg-violet-500/15 text-violet-400" },
  irc: { label: "IRC", color: "bg-orange-500/15 text-orange-400" },
  line: { label: "LINE", color: "bg-green-600/15 text-green-500" },
  nostr: { label: "Nostr", color: "bg-purple-600/15 text-purple-400" },
  mattermost: { label: "Mattermost", color: "bg-blue-600/15 text-blue-400" },
  zalo: { label: "Zalo", color: "bg-blue-500/15 text-blue-400" },
};

const inputCls = "w-full h-9 rounded-lg border border-white/[0.08] bg-white/[0.03] px-3 text-[13px] text-foreground placeholder:text-muted-foreground/45 focus:outline-none focus:ring-1 focus:ring-sky-400/40";
const FEISHU_DEFAULT_ACCOUNT_SCOPE = "__default__";

function getChannelInfo(ch: string) {
  return CHANNEL_LABELS[ch] ?? { label: ch, color: "bg-white/10 text-foreground/70" };
}

function normalizeFeishuRouteScope(value: string | null | undefined): FeishuRouteScope {
  if (value === "group" || value === "channel") {
    return "group";
  }
  if (value === "direct" || value === "dm") {
    return "direct";
  }
  return "account";
}

function createEmptyFeishuRoute(agentId = "", accountId?: string | null): FeishuMultiAgentRoute {
  return {
    agentId,
    scope: "account",
    accountId: accountId?.trim() ? accountId.trim() : null,
    peerId: "",
  };
}

function normalizeFeishuRoute(route: FeishuMultiAgentRoute): FeishuMultiAgentRoute {
  return {
    agentId: route.agentId?.trim() ?? "",
    scope: normalizeFeishuRouteScope(route.scope),
    accountId: route.accountId?.trim() ? route.accountId.trim() : null,
    peerId: route.peerId?.trim() ? route.peerId.trim() : "",
  };
}

function parseJsonValue<T>(raw: string, fallback: T): T {
  const trimmed = raw.trim();
  if (!trimmed) {
    return fallback;
  }

  try {
    return JSON.parse(trimmed) as T;
  } catch {
    for (let index = 0; index < trimmed.length; index += 1) {
      const ch = trimmed[index];
      if (ch !== "{" && ch !== "[") {
        continue;
      }
      try {
        return JSON.parse(trimmed.slice(index)) as T;
      } catch {
        continue;
      }
    }
    return fallback;
  }
}

function normalizeStatusMessage(value: Record<string, unknown>) {
  const messageFields = ["message", "detail", "error", "reason", "lastError", "status"];
  for (const field of messageFields) {
    const current = value[field];
    if (typeof current === "string" && current.trim()) {
      return current.trim();
    }
  }
  return "";
}

function inferStatusStateFromText(statusText: string): ChannelStatus["state"] {
  const normalized = statusText.trim().toLowerCase();
  if (!normalized) {
    return "configured";
  }
  if (/^(ok|online|connected|ready|running|healthy|works|linked)$/i.test(normalized)) {
    return "online";
  }
  if (/^(configured|enabled|setup|pending|idle)$/i.test(normalized)) {
    return "configured";
  }
  if (/^(disabled|stopped)$/i.test(normalized)) {
    return "disabled";
  }
  if (/(offline|disconnected|failed|error|warning|unreachable|cooldown|degraded|not linked|probe failed|audit failed)/i.test(normalized)) {
    return "issue";
  }
  return "configured";
}

function normalizeStatusState(value: Record<string, unknown>, statusText: string): ChannelStatus["state"] {
  const probe = value.probe && typeof value.probe === "object" && !Array.isArray(value.probe)
    ? value.probe as Record<string, unknown>
    : null;
  const audit = value.audit && typeof value.audit === "object" && !Array.isArray(value.audit)
    ? value.audit as Record<string, unknown>
    : null;

  const positiveFlags = [
    value.ok,
    value.connected,
    value.running,
    value.linked,
    value.healthy,
    value.available,
    value.success,
    probe?.ok,
  ];
  if (positiveFlags.some((entry) => entry === true)) {
    return "online";
  }

  if (value.enabled === false) {
    return "disabled";
  }

  const negativeFlags = [
    value.ok,
    value.connected,
    value.running,
    value.linked,
    value.healthy,
    value.available,
    value.success,
    probe?.ok,
    audit?.ok,
  ];
  const hasExplicitIssue = negativeFlags.some((entry) => entry === false)
    || (typeof value.lastError === "string" && value.lastError.trim().length > 0);

  if (value.configured === true || value.enabled === true) {
    return hasExplicitIssue ? "issue" : "configured";
  }

  const inferred = inferStatusStateFromText(statusText);
  if (inferred !== "configured") {
    return inferred;
  }

  return hasExplicitIssue ? "issue" : "configured";
}

function describeChannelStatus(state: ChannelStatus["state"], message: string) {
  if (message) {
    if (state === "configured" && /^(configured|enabled)$/i.test(message)) {
      return "已配置，等待状态检测";
    }
    return message;
  }

  if (state === "online") {
    return "在线";
  }
  if (state === "disabled") {
    return "已禁用";
  }
  if (state === "issue") {
    return "需要检查连接";
  }
  return "已配置，等待状态检测";
}

function getChannelStatusMeta(state: ChannelStatus["state"]) {
  if (state === "online") {
    return {
      label: "在线",
      className: "text-emerald-400",
      icon: <Wifi size={10} />,
    };
  }
  if (state === "disabled") {
    return {
      label: "已禁用",
      className: "text-muted-foreground",
      icon: <WifiOff size={10} />,
    };
  }
  if (state === "issue") {
    return {
      label: "需检查",
      className: "text-amber-300",
      icon: <WifiOff size={10} />,
    };
  }
  return {
    label: "已配置",
    className: "text-sky-300",
    icon: <Radio size={10} />,
  };
}

function normalizeChannelStatusRecord(channel: string, account: string, value: unknown): ChannelStatus | null {
  if (!channel.trim()) {
    return null;
  }

  if (typeof value === "boolean") {
    return {
      channel,
      account: account || "default",
      state: value ? "online" : "issue",
      message: value ? "在线" : "需要检查连接",
    };
  }

  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) {
      return null;
    }
    return {
      channel,
      account: account || "default",
      state: inferStatusStateFromText(trimmed),
      message: describeChannelStatus(inferStatusStateFromText(trimmed), trimmed),
    };
  }

  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }

  const record = value as Record<string, unknown>;
  const resolvedChannel =
    (typeof record.channel === "string" && record.channel.trim()) ? record.channel.trim() : channel;
  const resolvedAccount =
    (typeof record.account === "string" && record.account.trim())
      ? record.account.trim()
      : (typeof record.accountId === "string" && record.accountId.trim())
        ? record.accountId.trim()
        : account || "default";
  const statusText = typeof record.status === "string" ? record.status.trim() : "";
  const state = normalizeStatusState(record, statusText);
  const message = describeChannelStatus(state, normalizeStatusMessage(record));

  return {
    channel: resolvedChannel,
    account: resolvedAccount,
    state,
    message,
  };
}

function normalizeChannelStatuses(raw: unknown): ChannelStatus[] {
  const items: ChannelStatus[] = [];
  const seen = new Set<string>();

  const push = (entry: ChannelStatus | null) => {
    if (!entry) {
      return;
    }
    const key = `${entry.channel}:${entry.account}`;
    if (seen.has(key)) {
      return;
    }
    seen.add(key);
    items.push(entry);
  };

  if (Array.isArray(raw)) {
    for (const entry of raw) {
      const parsed = normalizeChannelStatusRecord(
        typeof entry?.channel === "string" ? entry.channel : "",
        typeof entry?.account === "string" ? entry.account : "default",
        entry,
      );
      push(parsed);
    }
    return items;
  }

  if (!raw || typeof raw !== "object") {
    return items;
  }

  const data = raw as Record<string, unknown>;
  const channelAccounts =
    data.channelAccounts && typeof data.channelAccounts === "object" && !Array.isArray(data.channelAccounts)
      ? data.channelAccounts as Record<string, unknown>
      : {};
  const defaultAccounts =
    data.channelDefaultAccountId && typeof data.channelDefaultAccountId === "object" && !Array.isArray(data.channelDefaultAccountId)
      ? data.channelDefaultAccountId as Record<string, unknown>
      : {};
  const channels =
    data.channels && typeof data.channels === "object" && !Array.isArray(data.channels)
      ? data.channels as Record<string, unknown>
      : {};

  for (const [channelName, accountEntries] of Object.entries(channelAccounts)) {
    if (Array.isArray(accountEntries)) {
      for (const entry of accountEntries) {
        push(normalizeChannelStatusRecord(
          channelName,
          typeof entry?.accountId === "string" ? entry.accountId : "default",
          entry,
        ));
      }
      continue;
    }
    if (!accountEntries || typeof accountEntries !== "object") {
      continue;
    }
    for (const [accountId, entry] of Object.entries(accountEntries as Record<string, unknown>)) {
      push(normalizeChannelStatusRecord(channelName, accountId, entry));
    }
  }

  for (const [channelName, entry] of Object.entries(channels)) {
    if (entry && typeof entry === "object" && !Array.isArray(entry)) {
      const nestedAccounts = (entry as Record<string, unknown>).accounts;
      if (nestedAccounts && typeof nestedAccounts === "object" && !Array.isArray(nestedAccounts)) {
        for (const [accountId, accountEntry] of Object.entries(nestedAccounts as Record<string, unknown>)) {
          push(normalizeChannelStatusRecord(channelName, accountId, accountEntry));
        }
        continue;
      }
    }

    const defaultAccount =
      typeof defaultAccounts[channelName] === "string" && (defaultAccounts[channelName] as string).trim()
        ? (defaultAccounts[channelName] as string).trim()
        : "default";
    push(normalizeChannelStatusRecord(channelName, defaultAccount, entry));
  }

  return items;
}

function formatRemainingSeconds(value: number) {
  const minutes = Math.floor(value / 60);
  const seconds = value % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

function extractChannelEntries(data: Record<string, unknown>) {
  const entries: ChannelEntry[] = [];
  const chat = (data.chat ?? {}) as Record<string, unknown>;

  for (const [channelName, accounts] of Object.entries(chat)) {
    if (!accounts || typeof accounts !== "object") {
      continue;
    }
    for (const [accountId, accountData] of Object.entries(accounts as Record<string, unknown>)) {
      const account = (accountData ?? {}) as Record<string, unknown>;
      entries.push({
        channel: channelName,
        account: accountId,
        name: (account.name as string) ?? accountId,
        enabled: account.enabled !== false,
      });
    }
  }

  entries.sort((left, right) => {
    if (left.channel !== right.channel) {
      return left.channel.localeCompare(right.channel);
    }
    return left.account.localeCompare(right.account);
  });

  return entries;
}

export default function ChannelsPage() {
  const [channels, setChannels] = useState<ChannelEntry[]>([]);
  const [statuses, setStatuses] = useState<ChannelStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [checkingStatus, setCheckingStatus] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState("");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingAccountId, setEditingAccountId] = useState<string | null>(null);
  const [feishuStatus, setFeishuStatus] = useState<FeishuPluginStatus | null>(null);
  const [setupLoading, setSetupLoading] = useState(false);
  const [setupError, setSetupError] = useState("");
  const [setupStep, setSetupStep] = useState<FeishuSetupStep>("install");

  const [installPhase, setInstallPhase] = useState<"idle" | "running" | "done">("idle");
  const [installProgress, setInstallProgress] = useState(0);
  const [installSuccess, setInstallSuccess] = useState(false);
  const [installLogs, setInstallLogs] = useState<LogEntry[]>([]);

  const [authSession, setAuthSession] = useState<FeishuAuthStartPayload | null>(null);
  const [authQrDataUrl, setAuthQrDataUrl] = useState("");
  const [bindingPhase, setBindingPhase] = useState<"idle" | "waiting" | "finalizing" | "done">("idle");
  const [bindingError, setBindingError] = useState("");
  const [bindingHint, setBindingHint] = useState("");
  const [existingBindingForm, setExistingBindingForm] = useState<ExistingFeishuBindingForm>({
    appId: "",
    appSecret: "",
    domain: "feishu",
  });
  const [existingBinding, setExistingBinding] = useState(false);
  const [existingBindingError, setExistingBindingError] = useState("");
  const [availableAgents, setAvailableAgents] = useState<AgentInfo[]>([]);
  const [feishuRoutes, setFeishuRoutes] = useState<FeishuMultiAgentRoute[]>([]);
  const [feishuDefaultAccountId, setFeishuDefaultAccountId] = useState("default");
  const [routesLoading, setRoutesLoading] = useState(false);
  const [routesSaving, setRoutesSaving] = useState(false);
  const [routesError, setRoutesError] = useState("");
  const [routesHint, setRoutesHint] = useState("");

  const installProgressRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const installLogEndRef = useRef<HTMLDivElement | null>(null);

  const appendInstallLog = useCallback((level: LogEntry["level"], message: string) => {
    setInstallLogs((prev) => [...prev, { timestamp: new Date(), level, message }]);
  }, []);

  useEffect(() => {
    installLogEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [installLogs]);

  useEffect(() => () => {
    if (installProgressRef.current) {
      clearInterval(installProgressRef.current);
    }
  }, []);

  useEffect(() => {
    const unlisten = listen<FeishuInstallEvent>("feishu-plugin-log", (event) => {
      const { level, message } = event.payload;

      if (level === "done") {
        if (installProgressRef.current) {
          clearInterval(installProgressRef.current);
        }
        const ok = message === "success";
        setInstallProgress(ok ? 100 : 0);
        setInstallSuccess(ok);
        setInstallPhase("done");
        if (ok) {
          appendInstallLog("success", "官方飞书插件安装完成");
        } else {
          appendInstallLog("error", "飞书插件安装失败，请检查日志");
        }
        return;
      }

      if (message.trim()) {
        const logLevel = level === "error" ? "error" : level === "warn" ? "warn" : "info";
        appendInstallLog(logLevel, message);
      }

      setInstallProgress((progress) => (progress < 92 ? progress + Math.random() * 3 : progress));
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appendInstallLog]);

  const feishuAccounts = useMemo(() => {
    const seen = new Set<string>();
    return channels
      .filter((entry) => entry.channel === "feishu")
      .filter((entry) => {
        const accountId = entry.account.trim();
        if (!accountId || seen.has(accountId)) {
          return false;
        }
        seen.add(accountId);
        return true;
      });
  }, [channels]);

  const routeAgentOptions = useMemo(() => {
    const options = new Map<string, string>();

    for (const agent of availableAgents) {
      options.set(agent.id, agent.name || agent.id);
    }

    for (const route of feishuRoutes) {
      const routeAgentId = route.agentId.trim();
      if (routeAgentId && !options.has(routeAgentId)) {
        options.set(routeAgentId, `${routeAgentId}（已缺失）`);
      }
    }

    return Array.from(options.entries()).map(([id, label]) => ({ id, label }));
  }, [availableAgents, feishuRoutes]);

  const routeAccountOptions = useMemo(() => {
    const options = new Map<string, string>();

    for (const account of feishuAccounts) {
      options.set(
        account.account,
        `${account.name} (${account.account}${account.account === feishuDefaultAccountId ? "，当前默认" : ""})`,
      );
    }

    for (const route of feishuRoutes) {
      const accountId = route.accountId?.trim();
      if (accountId && accountId !== "*" && !options.has(accountId)) {
        options.set(accountId, `${accountId}（已缺失）`);
      }
    }

    return Array.from(options.entries()).map(([id, label]) => ({ id, label }));
  }, [feishuAccounts, feishuDefaultAccountId, feishuRoutes]);

  const loadFeishuMultiAgentBindings = useCallback(async () => {
    setRoutesLoading(true);
    setRoutesError("");

    try {
      const [bindingsResult, agentList] = await Promise.all([
        invoke("get_feishu_multi_agent_bindings") as Promise<CommandResult>,
        invoke("list_agents") as Promise<AgentInfo[]>,
      ]);

      setAvailableAgents(Array.isArray(agentList) ? agentList : []);

      if (!bindingsResult.success) {
        setRoutesError(bindingsResult.stderr || "飞书多 Agent 路由读取失败");
        return;
      }

      const snapshot = parseJsonValue<FeishuMultiAgentBindingsSnapshot | null>(bindingsResult.stdout, null);
      if (!snapshot) {
        setRoutesError("飞书多 Agent 路由解析失败");
        return;
      }

      setFeishuDefaultAccountId(snapshot.defaultAccountId?.trim() || "default");
      setFeishuRoutes((snapshot.routes ?? []).map((route) => normalizeFeishuRoute(route)));
    } catch (e) {
      setRoutesError(`${e}`);
      setAvailableAgents([]);
    } finally {
      setRoutesLoading(false);
    }
  }, []);

  const fetchChannels = useCallback(async (options?: { silent?: boolean }) => {
    if (!options?.silent) {
      setLoading(true);
    }
    setError("");
    try {
      const result: CommandResult = await invoke("list_channels_snapshot");
      if (!result.success) {
        if (!options?.silent) {
          setChannels([]);
        }
        setError(result.stderr || "频道列表加载失败");
        return;
      }

      const data = result.stdout ? parseJsonValue<Record<string, unknown>>(result.stdout, {}) : {};
      setChannels(extractChannelEntries(data));
    } catch (e) {
      if (!options?.silent) {
        setChannels([]);
      }
      setError(`${e}`);
    } finally {
      if (!options?.silent) {
        setLoading(false);
      }
    }
  }, []);

  const fetchStatus = useCallback(async () => {
    setCheckingStatus(true);
    try {
      const result: CommandResult = await invoke("get_channel_status");
      if (!result.success) {
        setError((prev) => prev || result.stderr || "频道状态检查失败");
        return;
      }

      const data = result.stdout ? parseJsonValue<unknown>(result.stdout, []) : [];
      setStatuses(normalizeChannelStatuses(data));
    } catch (e) {
      setError((prev) => prev || `${e}`);
    } finally {
      setCheckingStatus(false);
    }
  }, []);

  const refreshAll = useCallback(async (options?: { silent?: boolean }) => {
    await Promise.all([fetchChannels({ silent: options?.silent }), fetchStatus()]);
  }, [fetchChannels, fetchStatus]);

  useEffect(() => {
    let cancelled = false;

    const bootstrap = async () => {
      setLoading(true);
      setError("");

      try {
        const result: CommandResult = await invoke("list_channels_snapshot");
        if (cancelled) {
          return;
        }

        if (!result.success) {
          setError(result.stderr || "频道列表加载失败");
        } else {
          const data = result.stdout ? parseJsonValue<Record<string, unknown>>(result.stdout, {}) : {};
          setChannels(extractChannelEntries(data));
        }
      } catch (e) {
        if (!cancelled) {
          setError(`${e}`);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }

      void refreshAll({ silent: true });
    };

    void bootstrap();

    return () => {
      cancelled = true;
    };
  }, [refreshAll]);

  const getStatus = useCallback((channel: string, account: string) => (
    statuses.find((entry) => entry.channel === channel && (entry.account === account || entry.account === "default"))
  ), [statuses]);

  const resetFeishuDialogState = useCallback(() => {
    setSetupError("");
    setSetupStep("install");
    setFeishuStatus(null);
    setInstallPhase("idle");
    setInstallProgress(0);
    setInstallSuccess(false);
    setInstallLogs([]);
    setAuthSession(null);
    setAuthQrDataUrl("");
    setBindingPhase("idle");
    setBindingError("");
    setBindingHint("");
    setExistingBinding(false);
    setExistingBindingError("");
    setExistingBindingForm({
      appId: "",
      appSecret: "",
      domain: "feishu",
    });
    setAvailableAgents([]);
    setFeishuRoutes([]);
    setFeishuDefaultAccountId("default");
    setRoutesLoading(false);
    setRoutesSaving(false);
    setRoutesError("");
    setRoutesHint("");
  }, []);

  const loadFeishuSetup = useCallback(async () => {
    setSetupLoading(true);
    setSetupError("");
    try {
      const result: CommandResult = await invoke("get_feishu_plugin_status");
      if (!result.success) {
        setSetupError(result.stderr || "飞书插件状态读取失败");
        return null;
      }

      const parsed = parseJsonValue<FeishuPluginStatus | null>(result.stdout, null);
      if (!parsed) {
        setSetupError("飞书插件状态解析失败");
        return null;
      }

      setFeishuStatus(parsed);
      setSetupStep(parsed.officialPluginInstalled ? (parsed.channelConfigured ? "done" : "bind") : "install");
      setExistingBindingForm((prev) => ({
        appId: parsed.appId || prev.appId,
        appSecret: "",
        domain: parsed.domain === "lark" ? "lark" : "feishu",
      }));
      return parsed;
    } catch (e) {
      setSetupError(`${e}`);
      return null;
    } finally {
      setSetupLoading(false);
    }
  }, []);

  const applyFeishuBindingSuccess = useCallback((message: string, next: { appId: string; domain: string }) => {
    const resolvedDomain = next.domain === "lark" ? "lark" : "feishu";

    setSetupError("");
    setSetupStep("done");
    setBindingPhase("done");
    setBindingError("");
    setExistingBindingError("");
    setBindingHint(message);
    setAuthSession(null);
    setAuthQrDataUrl("");
    setFeishuStatus((prev) => ({
      officialPluginInstalled: true,
      officialPluginEnabled: true,
      communityPluginEnabled: prev?.communityPluginEnabled ?? false,
      channelConfigured: true,
      appId: next.appId || prev?.appId || "",
      displayName: prev?.displayName || "飞书官方插件",
      domain: resolvedDomain,
    }));
    setExistingBindingForm((prev) => ({
      appId: next.appId || prev.appId,
      appSecret: "",
      domain: resolvedDomain,
    }));

    void (async () => {
      try {
        const result: CommandResult = await invoke("get_feishu_plugin_status");
        if (result.success) {
          const parsed = parseJsonValue<FeishuPluginStatus | null>(result.stdout, null);
          if (parsed) {
            setFeishuStatus(parsed);
            setExistingBindingForm((prev) => ({
              appId: parsed.appId || prev.appId,
              appSecret: "",
              domain: parsed.domain === "lark" ? "lark" : "feishu",
            }));
          }
        }
      } catch {
        // Keep the optimistic UI if background status sync fails.
      }

      try {
        await refreshAll({ silent: true });
      } catch {
        // The page-level refresh already manages its own error state.
      }

      try {
        await loadFeishuMultiAgentBindings();
      } catch {
        // Route loading already owns its local error state.
      }
    })();
  }, [loadFeishuMultiAgentBindings, refreshAll]);

  const beginFeishuBinding = useCallback(async () => {
    setSetupError("");
    setBindingError("");
    setExistingBindingError("");
    setBindingHint("");
    setBindingPhase("idle");
    setAuthSession(null);
    setAuthQrDataUrl("");

    try {
      const result: CommandResult = await invoke("start_feishu_auth_session", {
        env: "prod",
        lane: null,
      });
      if (!result.success) {
        setBindingError(result.stderr || "飞书扫码初始化失败");
        return;
      }

      const payload = parseJsonValue<FeishuAuthStartPayload | null>(result.stdout, null);
      if (!payload) {
        setBindingError("飞书扫码初始化失败");
        return;
      }

      const dataUrl = await QRCode.toDataURL(payload.verificationUrl, {
        errorCorrectionLevel: "M",
        margin: 1,
        width: 280,
      });

      setAuthSession(payload);
      setAuthQrDataUrl(dataUrl);
      setBindingPhase("waiting");
      setBindingHint("请用飞书扫一扫，绑定完成后当前窗口会自动继续。");
    } catch (e) {
      setBindingError(`${e}`);
    }
  }, []);

  const openFeishuDialog = useCallback(async (accountId?: string) => {
    setEditingAccountId(accountId ?? null);
    setDialogOpen(true);
    resetFeishuDialogState();
    await Promise.all([loadFeishuSetup(), loadFeishuMultiAgentBindings()]);
  }, [loadFeishuMultiAgentBindings, loadFeishuSetup, resetFeishuDialogState]);

  const closeDialog = useCallback(() => {
    if (installPhase === "running" || bindingPhase === "finalizing" || existingBinding || routesSaving) {
      return;
    }
    setDialogOpen(false);
    setEditingAccountId(null);
    resetFeishuDialogState();
  }, [bindingPhase, existingBinding, installPhase, resetFeishuDialogState, routesSaving]);

  const handleRemove = async (channel: string, account: string) => {
    if (!confirm(`确定移除 ${getChannelInfo(channel).label} (${account}) 吗？`)) return;
    const key = `${channel}:${account}`;
    setRemoving(key);
    try {
      const result: CommandResult = await invoke("remove_channel", { channel, account });
      if (!result.success) {
        alert(result.stderr || "移除频道失败");
        return;
      }
      setChannels((prev) => prev.filter((entry) => !(entry.channel === channel && entry.account === account)));
      setStatuses((prev) => prev.filter((entry) => !(entry.channel === channel && entry.account === account)));
    } catch (e) {
      alert(`移除频道失败: ${e}`);
    } finally {
      setRemoving(null);
    }
  };

  const handleInstallFeishu = useCallback(async () => {
    setInstallPhase("running");
    setInstallSuccess(false);
    setInstallProgress(5);
    setInstallLogs([]);
    setBindingError("");
    setExistingBindingError("");
    setSetupError("");

    appendInstallLog("info", "正在应用内安装飞书官方插件...");
    appendInstallLog("info", "安装完成后你可以扫码创建新机器人，或填写已有机器人的 App ID / App Secret。");

    if (installProgressRef.current) {
      clearInterval(installProgressRef.current);
    }
    installProgressRef.current = setInterval(() => {
      setInstallProgress((progress) => (progress < 85 ? progress + 1 : progress));
    }, 1500);

    try {
      const result: CommandResult = await invoke("install_feishu_plugin");
      if (!result.success) {
        setSetupError(result.stderr || "飞书官方插件安装失败");
        return;
      }

      const latestStatus = await loadFeishuSetup();
      if (latestStatus?.officialPluginInstalled) {
        setSetupStep("bind");
        setBindingHint("官方插件已安装好。你现在可以扫码创建新机器人，或填写已有机器人的 App ID / App Secret。");
      }
    } catch (e) {
      if (installProgressRef.current) {
        clearInterval(installProgressRef.current);
      }
      setInstallPhase("done");
      setInstallProgress(0);
      setInstallSuccess(false);
      setSetupError(`${e}`);
      appendInstallLog("error", `${e}`);
    }
  }, [appendInstallLog, loadFeishuSetup]);

  const handleBindExistingFeishu = useCallback(async () => {
    const appId = existingBindingForm.appId.trim();
    const appSecret = existingBindingForm.appSecret.trim();

    if (!appId) {
      setExistingBindingError("请先填写飞书 App ID");
      return;
    }
    if (!appSecret) {
      setExistingBindingError("请先填写飞书 App Secret");
      return;
    }

    setSetupError("");
    setBindingError("");
    setExistingBindingError("");
    setExistingBinding(true);
    setBindingPhase("idle");
    setAuthSession(null);
    setAuthQrDataUrl("");
    setBindingHint("正在校验已有机器人的凭证并写入配置...");

    try {
      const result: CommandResult = await invoke("bind_existing_feishu_app", {
        appId,
        appSecret,
        domain: existingBindingForm.domain,
      });

      if (!result.success) {
        setExistingBindingError(result.stderr || "已有飞书机器人绑定失败");
        return;
      }

      applyFeishuBindingSuccess(
        result.stdout || "已有飞书机器人绑定完成，可以回到频道列表继续使用。",
        {
          appId,
          domain: existingBindingForm.domain,
        },
      );
    } catch (e) {
      setExistingBindingError(`${e}`);
    } finally {
      setExistingBinding(false);
    }
  }, [applyFeishuBindingSuccess, existingBindingForm]);

  const handleAddFeishuRoute = useCallback(() => {
    setRoutesError("");
    setRoutesHint("");
    setFeishuRoutes((prev) => [
      ...prev,
      createEmptyFeishuRoute(routeAgentOptions[0]?.id ?? "", editingAccountId ?? null),
    ]);
  }, [editingAccountId, routeAgentOptions]);

  const handleUpdateFeishuRoute = useCallback((
    index: number,
    patch: Partial<FeishuMultiAgentRoute>,
  ) => {
    setRoutesError("");
    setRoutesHint("");
    setFeishuRoutes((prev) => prev.map((route, routeIndex) => {
      if (routeIndex !== index) {
        return route;
      }

      const next = normalizeFeishuRoute({ ...route, ...patch });
      if (next.scope === "account") {
        next.peerId = "";
      }
      return next;
    }));
  }, []);

  const handleMoveFeishuRoute = useCallback((index: number, direction: -1 | 1) => {
    setRoutesError("");
    setRoutesHint("");
    setFeishuRoutes((prev) => {
      const nextIndex = index + direction;
      if (nextIndex < 0 || nextIndex >= prev.length) {
        return prev;
      }

      const next = [...prev];
      const [current] = next.splice(index, 1);
      next.splice(nextIndex, 0, current);
      return next;
    });
  }, []);

  const handleRemoveFeishuRoute = useCallback((index: number) => {
    setRoutesError("");
    setRoutesHint("");
    setFeishuRoutes((prev) => prev.filter((_, routeIndex) => routeIndex !== index));
  }, []);

  const handleSaveFeishuRoutes = useCallback(async () => {
    const normalizedRoutes = feishuRoutes.map((route) => normalizeFeishuRoute(route));

    if (normalizedRoutes.some((route) => !route.agentId.trim())) {
      setRoutesError("每条路由都需要选择一个 Agent");
      return;
    }

    const missingPeerRoute = normalizedRoutes.find((route) => route.scope !== "account" && !route.peerId?.trim());
    if (missingPeerRoute) {
      setRoutesError(missingPeerRoute.scope === "group" ? "群组路由需要填写群组 ID" : "私聊路由需要填写用户 Open ID");
      return;
    }

    setRoutesSaving(true);
    setRoutesError("");
    setRoutesHint("");

    try {
      const result: CommandResult = await invoke("save_feishu_multi_agent_bindings", {
        routes: normalizedRoutes.map((route) => ({
          agentId: route.agentId.trim(),
          scope: route.scope,
          accountId: route.accountId?.trim() ? route.accountId.trim() : null,
          peerId: route.scope === "account" ? null : (route.peerId?.trim() || null),
        })),
      });

      if (!result.success) {
        setRoutesError(result.stderr || "飞书多 Agent 路由保存失败");
        return;
      }

      setRoutesHint(result.stdout || "飞书多 Agent 路由已保存");
      await Promise.all([
        loadFeishuMultiAgentBindings(),
        refreshAll({ silent: true }),
      ]);
    } catch (e) {
      setRoutesError(`${e}`);
    } finally {
      setRoutesSaving(false);
    }
  }, [feishuRoutes, loadFeishuMultiAgentBindings, refreshAll]);

  useEffect(() => {
    if (bindingPhase !== "waiting" || !authSession) {
      return undefined;
    }

    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const scheduleNext = (seconds: number) => {
      timer = setTimeout(() => {
        void poll();
      }, seconds * 1000);
    };

    const poll = async () => {
      if (cancelled) {
        return;
      }

      try {
        const result: CommandResult = await invoke("poll_feishu_auth_session", {
          deviceCode: authSession.deviceCode,
          env: authSession.env,
          lane: null,
          domain: authSession.domain,
        });
        if (cancelled) {
          return;
        }

        if (!result.success) {
          setBindingPhase("idle");
          setBindingError(result.stderr || "飞书扫码状态获取失败");
          return;
        }

        const payload = parseJsonValue<FeishuAuthPollPayload | null>(result.stdout, null);
        if (!payload) {
          setBindingPhase("idle");
          setBindingError("飞书扫码状态解析失败");
          return;
        }

        if (payload.suggestedDomain && payload.suggestedDomain !== authSession.domain) {
          setAuthSession((prev) => prev ? { ...prev, domain: payload.suggestedDomain ?? prev.domain } : prev);
          setBindingHint(payload.suggestedDomain === "lark" ? "已切换到 Lark 域继续等待扫码结果..." : "继续等待扫码结果...");
          scheduleNext(1);
          return;
        }

        if (payload.status === "success" && payload.appId && payload.appSecret) {
          setBindingPhase("finalizing");
          setBindingHint("扫码成功，正在应用绑定结果...");

          const bindingResult: CommandResult = await invoke("complete_feishu_plugin_binding", {
            appId: payload.appId,
            appSecret: payload.appSecret,
            domain: payload.suggestedDomain ?? authSession.domain,
            openId: payload.openId ?? null,
          });
          if (cancelled) {
            return;
          }

          if (!bindingResult.success) {
            setBindingPhase("idle");
            setBindingError(bindingResult.stderr || "飞书绑定失败");
            return;
          }

          applyFeishuBindingSuccess(
            bindingResult.stdout || "飞书已绑定完成，可以回到频道列表继续使用。",
            {
              appId: payload.appId,
              domain: payload.suggestedDomain ?? authSession.domain,
            },
          );
          return;
        }

        if (payload.status === "pending") {
          scheduleNext(authSession.intervalSeconds);
          return;
        }

        if (payload.status === "slow_down") {
          setBindingHint("飞书正在处理授权，请稍等片刻...");
          scheduleNext(authSession.intervalSeconds + 5);
          return;
        }

        setBindingPhase("idle");
        if (payload.status === "denied") {
          setBindingError("你在飞书里取消了授权，请重新扫码。");
        } else if (payload.status === "expired") {
          setBindingError("扫码已过期，请重新生成二维码。");
        } else {
          setBindingError(payload.errorDescription || payload.error || "飞书扫码失败，请重试。");
        }
      } catch (e) {
        if (!cancelled) {
          setBindingPhase("idle");
          setBindingError(`${e}`);
        }
      }
    };

    void poll();

    return () => {
      cancelled = true;
      if (timer) {
        clearTimeout(timer);
      }
    };
  }, [applyFeishuBindingSuccess, authSession, bindingPhase]);

  const showInstallLogs = installPhase === "running" || installLogs.length > 0;

  return (
    <TooltipProvider delayDuration={300}>
      <PageShell
        header={(
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-violet-500/10">
                <MessageSquare size={15} className="text-violet-400" />
              </div>
              <div>
                <h2 className="text-sm font-semibold">频道管理</h2>
                <p className="text-[11px] text-muted-foreground">
                  {loading ? "加载中" : `${channels.length} 个频道`}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => void refreshAll()} disabled={loading || checkingStatus}>
                    {loading || checkingStatus ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>刷新</TooltipContent>
              </Tooltip>
              <Button size="sm" variant="outline" onClick={() => void openFeishuDialog()}>
                <Plus size={14} /> 添加飞书
              </Button>
            </div>
          </div>
        )}
      >
        {error && (
          <div className="rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-[12px] text-amber-300">
            {error}
          </div>
        )}

        {loading ? (
          <div className="flex items-center justify-center py-20 text-muted-foreground">
            <Loader2 size={18} className="mr-2 animate-spin" />
            <span className="text-[13px]">加载中...</span>
          </div>
        ) : channels.length === 0 ? (
          <Card>
            <CardContent className="py-16 text-center">
              <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl bg-violet-500/10">
                <Radio size={22} className="text-violet-400" />
              </div>
              <h3 className="mb-1 text-[14px] font-semibold">先接入一个飞书频道</h3>
              <p className="mx-auto mb-4 max-w-sm text-[12px] text-muted-foreground">
                点击“添加飞书”后会直接在应用内检测并安装官方插件，接着可以扫码创建新机器人，或绑定已有机器人，不再跳出命令行窗口。
              </p>
              <div className="flex justify-center gap-2">
                <Button size="sm" onClick={() => void openFeishuDialog()}>
                  <Plus size={14} /> 添加飞书
                </Button>
              </div>
            </CardContent>
          </Card>
        ) : (
          <div className="space-y-3">
            {channels.map((channel) => {
              const info = getChannelInfo(channel.channel);
              const status = getStatus(channel.channel, channel.account);
              const statusMeta = status ? getChannelStatusMeta(status.state) : null;
              const key = `${channel.channel}:${channel.account}`;

              return (
                <Card key={key} className="group transition-colors hover:border-violet-500/20">
                  <CardContent className="p-4">
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex min-w-0 items-center gap-3">
                        <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-violet-500/10">
                          <MessageSquare size={16} className="text-violet-400" />
                        </div>
                        <div className="min-w-0">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="text-[13px] font-semibold">{channel.name}</span>
                            <Badge variant="secondary" className={`px-1.5 py-0 text-[10px] ${info.color}`}>
                              {info.label}
                            </Badge>
                            {status && statusMeta && (
                              <span className={`flex items-center gap-1 text-[10px] ${statusMeta.className}`}>
                                {statusMeta.icon}
                                {statusMeta.label}
                              </span>
                            )}
                            {checkingStatus && !status && (
                              <Loader2 size={10} className="animate-spin text-muted-foreground" />
                            )}
                          </div>
                          <p className="mt-0.5 text-[11px] text-muted-foreground">
                            账号: {channel.account}
                            {!channel.enabled && <span className="ml-2 text-amber-400">已禁用</span>}
                          </p>
                          {status?.message && (
                            <p className="mt-1 text-[11px] text-muted-foreground">{status.message}</p>
                          )}
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-1">
                        {channel.channel === "feishu" && (
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-7 w-7 text-muted-foreground opacity-0 transition-opacity hover:text-sky-300 group-hover:opacity-100"
                                onClick={() => void openFeishuDialog(channel.account)}
                              >
                                <Settings2 size={13} />
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>管理飞书官方插件</TooltipContent>
                          </Tooltip>
                        )}
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="h-7 w-7 text-muted-foreground opacity-0 transition-opacity hover:text-red-400 group-hover:opacity-100"
                              onClick={() => void handleRemove(channel.channel, channel.account)}
                              disabled={removing === key}
                            >
                              {removing === key ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent>移除频道</TooltipContent>
                        </Tooltip>
                      </div>
                    </div>
                  </CardContent>
                </Card>
              );
            })}
          </div>
        )}
      </PageShell>

      {dialogOpen && (
        <div className="fixed inset-0 z-[120] flex items-center justify-center bg-black/70 px-4 py-6 backdrop-blur-sm" onClick={closeDialog}>
          <Card className="w-full max-w-3xl border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40" onClick={(event) => event.stopPropagation()}>
            <CardContent className="max-h-[85vh] space-y-4 overflow-auto p-5">
              <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                <div className="space-y-1">
                  <div className="flex items-center gap-2">
                    <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-blue-500/10">
                      <MessageSquare size={15} className="text-blue-400" />
                    </div>
                    <div>
                      <h3 className="text-[13px] font-semibold">{editingAccountId ? "管理飞书接入" : "添加飞书"}</h3>
                      <p className="text-[11px] text-muted-foreground">
                        直接在应用内完成官方插件安装、扫码新建和已有机器人绑定，不再外跳终端。
                      </p>
                    </div>
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={closeDialog} disabled={installPhase === "running" || bindingPhase === "finalizing" || existingBinding || routesSaving}>
                  关闭
                </Button>
              </div>

              {setupLoading ? (
                <div className="flex items-center justify-center py-16 text-muted-foreground">
                  <Loader2 size={18} className="mr-2 animate-spin" />
                  <span className="text-[13px]">正在检查飞书插件状态...</span>
                </div>
              ) : (
                <>
                  {(setupError || bindingError || existingBindingError) && (
                    <div className="rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                      {setupError || bindingError || existingBindingError}
                    </div>
                  )}

                  <div className="grid gap-3 md:grid-cols-3">
                    <Card className={`border-white/[0.08] ${setupStep === "install" ? "bg-sky-500/10" : "bg-white/[0.02]"}`}>
                      <CardContent className="space-y-2 p-4">
                        <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 1 步</p>
                        <h4 className="text-[13px] font-semibold">安装官方插件</h4>
                        <p className="text-[12px] leading-5 text-muted-foreground">
                          自动检测本机是否已安装飞书官方插件，缺失时直接在应用内完成安装。
                        </p>
                      </CardContent>
                    </Card>
                    <Card className={`border-white/[0.08] ${setupStep === "bind" ? "bg-sky-500/10" : "bg-white/[0.02]"}`}>
                      <CardContent className="space-y-2 p-4">
                        <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 2 步</p>
                        <h4 className="text-[13px] font-semibold">绑定机器人</h4>
                        <p className="text-[12px] leading-5 text-muted-foreground">
                          可以在当前窗口扫码创建新机器人，也可以直接填写已有机器人的 App ID / App Secret。
                        </p>
                      </CardContent>
                    </Card>
                    <Card className={`border-white/[0.08] ${setupStep === "done" ? "bg-emerald-500/10" : "bg-white/[0.02]"}`}>
                      <CardContent className="space-y-2 p-4">
                        <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 3 步</p>
                        <h4 className="text-[13px] font-semibold">写入配置并后台生效</h4>
                        <p className="text-[12px] leading-5 text-muted-foreground">
                          绑定成功后会立即写入 `channels.feishu`，并在后台刷新网关和频道列表。
                        </p>
                      </CardContent>
                    </Card>
                  </div>

                  {setupStep === "install" && (
                    <Card className="border-white/[0.08] bg-white/[0.02]">
                      <CardContent className="space-y-4 p-4">
                        <div className="space-y-1">
                          <h4 className="text-[14px] font-semibold">先安装飞书官方插件</h4>
                          <p className="text-[12px] text-muted-foreground">
                            检测到当前环境还没有官方飞书插件。点一下按钮，应用会自动完成安装，装好后你就可以扫码创建新机器人，或绑定已有机器人。
                          </p>
                        </div>

                        {showInstallLogs && (
                          <div className="space-y-3">
                            <div>
                              <div className="mb-1 flex items-center justify-between text-[12px] text-muted-foreground">
                                <span>{installPhase === "running" ? "正在安装..." : installSuccess ? "安装完成" : "安装日志"}</span>
                                <span>{Math.round(installProgress)}%</span>
                              </div>
                              <div className="h-1.5 overflow-hidden rounded-full bg-white/[0.06]">
                                <div className="h-full rounded-full bg-sky-400 transition-all" style={{ width: `${installProgress}%` }} />
                              </div>
                            </div>

                            <Card className="border-white/[0.06] bg-[#060b11]">
                              <CardContent className="p-0">
                                <ScrollArea className="h-52">
                                  <div className="space-y-1 p-3 font-mono text-[11px]">
                                    {installLogs.map((log, index) => (
                                      <div key={`${log.timestamp.getTime()}-${index}`} className="flex gap-2 leading-5">
                                        <span className="w-[60px] shrink-0 text-muted-foreground/40">{log.timestamp.toLocaleTimeString()}</span>
                                        <span className={
                                          log.level === "error" ? "text-red-400"
                                            : log.level === "success" ? "text-emerald-400"
                                            : log.level === "warn" ? "text-amber-400"
                                            : "text-foreground/70"
                                        }
                                        >
                                          {log.message}
                                        </span>
                                      </div>
                                    ))}
                                    <div ref={installLogEndRef} />
                                  </div>
                                </ScrollArea>
                              </CardContent>
                            </Card>
                          </div>
                        )}

                        <div className="flex flex-wrap gap-2">
                          <Button size="sm" onClick={() => void handleInstallFeishu()} disabled={installPhase === "running"}>
                            {installPhase === "running" ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                            {installPhase === "running" ? "安装中..." : "一键安装并继续"}
                          </Button>
                        </div>
                      </CardContent>
                    </Card>
                  )}

                  {(setupStep === "bind" || setupStep === "done") && (
                    <div className="space-y-4">
                      <div className="grid gap-4 lg:grid-cols-[1.15fr_0.85fr]">
                        <Card className="border-white/[0.08] bg-white/[0.02]">
                          <CardContent className="space-y-4 p-4">
                            <div className="space-y-1">
                              <h4 className="text-[14px] font-semibold">
                                {setupStep === "done" ? "飞书已接入" : "绑定飞书机器人"}
                              </h4>
                              <p className="text-[12px] text-muted-foreground">
                                {setupStep === "done"
                                  ? "官方飞书插件已经可用。如果你想更换绑定，可以重新扫码创建新机器人，或切换到已有机器人。"
                                  : "这里支持两种接入方式：扫码创建新机器人，或输入已有机器人的 App ID / App Secret。"}
                              </p>
                            </div>

                            {feishuStatus?.channelConfigured && setupStep === "done" && (
                              <div className="rounded-xl border border-emerald-500/15 bg-emerald-500/8 p-3">
                                <div className="flex items-start gap-2">
                                  <CheckCircle2 size={16} className="mt-0.5 text-emerald-300" />
                                  <div className="text-[12px] text-emerald-100/90">
                                    <p>当前已绑定 {feishuStatus.displayName || "飞书官方插件"}。</p>
                                    <p className="mt-1 text-emerald-100/70">App ID: {feishuStatus.appId || "已写入配置"}，环境: {feishuStatus.domain === "lark" ? "Lark" : "飞书"}</p>
                                  </div>
                                </div>
                              </div>
                            )}

                            <div className="grid gap-4 xl:grid-cols-2">
                              <Card className="border-white/[0.06] bg-white/[0.03]">
                                <CardContent className="space-y-4 p-4">
                                  <div className="space-y-1">
                                    <h5 className="text-[13px] font-semibold">扫码创建新机器人</h5>
                                    <p className="text-[12px] text-muted-foreground">
                                      应用内生成二维码，用飞书扫一扫后自动写入配置。
                                    </p>
                                  </div>

                                  {bindingPhase === "waiting" && authQrDataUrl ? (
                                    <div className="space-y-3">
                                      <div className="rounded-2xl border border-sky-500/15 bg-sky-500/8 p-5 text-center">
                                        <img src={authQrDataUrl} alt="飞书扫码二维码" className="mx-auto h-56 w-56 rounded-xl bg-white p-3" />
                                        <p className="mt-3 text-[13px] font-medium text-sky-50">请用飞书扫一扫</p>
                                        <p className="mt-1 text-[12px] text-sky-100/75">{bindingHint}</p>
                                      </div>
                                      <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                                        <p>二维码有效期约 {authSession ? formatRemainingSeconds(authSession.expireInSeconds) : "--"}，授权完成后这里会自动继续。</p>
                                        <p className="mt-1">如果扫码后暂时没有变化，可以等待几秒，或者重新生成二维码。</p>
                                      </div>
                                    </div>
                                  ) : (
                                    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-4 text-[12px] text-muted-foreground">
                                      <p>{bindingHint || "准备好后点击下方按钮生成二维码。"}</p>
                                    </div>
                                  )}

                                  <div className="flex flex-wrap gap-2">
                                    <Button
                                      size="sm"
                                      onClick={() => void beginFeishuBinding()}
                                      disabled={bindingPhase === "waiting" || bindingPhase === "finalizing" || existingBinding || installPhase === "running"}
                                    >
                                      {bindingPhase === "finalizing" ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                                      {bindingPhase === "waiting" ? "等待扫码中..." : bindingPhase === "finalizing" ? "写入配置中..." : feishuStatus?.channelConfigured ? "重新扫码创建" : "开始扫码创建"}
                                    </Button>
                                    {bindingPhase === "waiting" && (
                                      <Button
                                        size="sm"
                                        variant="outline"
                                        onClick={() => {
                                          setAuthSession(null);
                                          setAuthQrDataUrl("");
                                          setBindingPhase("idle");
                                          setBindingError("");
                                          setBindingHint("已取消本次扫码。你可以重新生成二维码，或改用已有机器人绑定。");
                                        }}
                                        disabled={existingBinding}
                                      >
                                        取消扫码
                                      </Button>
                                    )}
                                  </div>
                                </CardContent>
                              </Card>

                              <Card className="border-white/[0.06] bg-white/[0.03]">
                                <CardContent className="space-y-4 p-4">
                                  <div className="space-y-1">
                                    <h5 className="text-[13px] font-semibold">绑定已有机器人</h5>
                                    <p className="text-[12px] text-muted-foreground">
                                      直接输入已创建机器人的 App ID 和 App Secret，无需重新扫码创建。
                                    </p>
                                  </div>

                                  <div className="space-y-3">
                                    <div className="space-y-1">
                                      <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">域名环境</label>
                                      <div className="grid grid-cols-2 gap-2">
                                        <Button
                                          type="button"
                                          size="sm"
                                          variant="outline"
                                          className={existingBindingForm.domain === "feishu"
                                            ? "border-sky-400/40 bg-sky-500/15 text-sky-50 hover:bg-sky-500/20 hover:text-sky-50"
                                            : "border-white/[0.08] bg-white/[0.02] text-muted-foreground hover:bg-white/[0.04]"}
                                          onClick={() => setExistingBindingForm((prev) => ({ ...prev, domain: "feishu" }))}
                                          disabled={existingBinding || bindingPhase === "finalizing"}
                                        >
                                          飞书
                                        </Button>
                                        <Button
                                          type="button"
                                          size="sm"
                                          variant="outline"
                                          className={existingBindingForm.domain === "lark"
                                            ? "border-sky-400/40 bg-sky-500/15 text-sky-50 hover:bg-sky-500/20 hover:text-sky-50"
                                            : "border-white/[0.08] bg-white/[0.02] text-muted-foreground hover:bg-white/[0.04]"}
                                          onClick={() => setExistingBindingForm((prev) => ({ ...prev, domain: "lark" }))}
                                          disabled={existingBinding || bindingPhase === "finalizing"}
                                        >
                                          Lark
                                        </Button>
                                      </div>
                                    </div>

                                    <div className="space-y-1">
                                      <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">App ID</label>
                                      <input
                                        className={inputCls}
                                        placeholder="cli_xxx"
                                        value={existingBindingForm.appId}
                                        onChange={(event) => setExistingBindingForm((prev) => ({ ...prev, appId: event.target.value }))}
                                        disabled={existingBinding || bindingPhase === "finalizing"}
                                        autoCapitalize="off"
                                        autoComplete="off"
                                        spellCheck={false}
                                      />
                                    </div>

                                    <div className="space-y-1">
                                      <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">App Secret</label>
                                      <input
                                        type="password"
                                        className={inputCls}
                                        placeholder="填写已有机器人的 App Secret"
                                        value={existingBindingForm.appSecret}
                                        onChange={(event) => setExistingBindingForm((prev) => ({ ...prev, appSecret: event.target.value }))}
                                        disabled={existingBinding || bindingPhase === "finalizing"}
                                        autoCapitalize="off"
                                        autoComplete="off"
                                        spellCheck={false}
                                      />
                                    </div>
                                  </div>

                                  <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                                    <p>如果你之前已经在飞书开放平台建好了机器人，这里直接填入凭证即可，不需要重新走扫码创建。</p>
                                  </div>

                                  <div className="flex flex-wrap gap-2">
                                    <Button
                                      size="sm"
                                      onClick={() => void handleBindExistingFeishu()}
                                      disabled={existingBinding || bindingPhase === "finalizing" || installPhase === "running" || !existingBindingForm.appId.trim() || !existingBindingForm.appSecret.trim()}
                                    >
                                      {existingBinding ? <Loader2 className="animate-spin" /> : <CheckCircle2 size={14} />}
                                      {existingBinding ? "绑定中..." : feishuStatus?.channelConfigured ? "保存并改绑" : "保存并绑定"}
                                    </Button>
                                    <Button
                                      size="sm"
                                      variant="outline"
                                      onClick={() => setExistingBindingForm((prev) => ({ ...prev, appSecret: "" }))}
                                      disabled={existingBinding || bindingPhase === "finalizing" || !existingBindingForm.appSecret}
                                    >
                                      清空密钥
                                    </Button>
                                  </div>
                                </CardContent>
                              </Card>
                            </div>

                            <div className="flex flex-wrap gap-2">
                              <Button size="sm" variant="outline" onClick={() => void refreshAll()} disabled={bindingPhase === "finalizing" || existingBinding}>
                                <RefreshCw size={14} />
                                刷新频道列表
                              </Button>
                            </div>
                          </CardContent>
                        </Card>

                        <Card className="border-white/[0.08] bg-white/[0.02]">
                          <CardContent className="space-y-4 p-4">
                            <div className="space-y-1">
                              <h4 className="text-[13px] font-semibold">当前状态</h4>
                              <p className="text-[12px] text-muted-foreground">这里会根据插件检测、安装和扫码结果实时更新。</p>
                            </div>

                            <div className="space-y-3 text-[12px]">
                              <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                                <p className="text-muted-foreground">官方插件</p>
                                <p className="mt-1 font-medium text-foreground">{feishuStatus?.officialPluginInstalled ? "已安装" : "未安装"}</p>
                              </div>
                              <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                                <p className="text-muted-foreground">官方插件状态</p>
                                <p className="mt-1 font-medium text-foreground">{feishuStatus?.officialPluginEnabled ? "已启用" : "待启用"}</p>
                              </div>
                              <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                                <p className="text-muted-foreground">机器人绑定</p>
                                <p className="mt-1 font-medium text-foreground">
                                  {existingBinding
                                    ? "校验已有机器人"
                                    : bindingPhase === "done"
                                      ? "已完成"
                                      : bindingPhase === "finalizing"
                                        ? "写入配置中"
                                        : bindingPhase === "waiting"
                                          ? "等待扫码"
                                          : feishuStatus?.channelConfigured
                                            ? "已配置，可重新扫码"
                                            : "尚未开始"}
                                </p>
                              </div>
                              {feishuStatus?.communityPluginEnabled && (
                                <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-3 text-amber-100/85">
                                  检测到旧的社区飞书插件仍配置为启用状态。应用内绑定会自动优先启用官方插件。
                                </div>
                              )}
                            </div>
                          </CardContent>
                        </Card>
                      </div>

                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                            <div className="space-y-1">
                              <div className="flex items-center gap-2">
                                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/10">
                                  <Route size={15} className="text-sky-300" />
                                </div>
                                <div>
                                  <h4 className="text-[13px] font-semibold">多 Agent 路由</h4>
                                  <p className="text-[12px] text-muted-foreground">
                                    按官方 `bindings` 规则，把飞书不同账号、私聊用户或群组定向到不同 Agent。
                                  </p>
                                </div>
                              </div>
                            </div>

                            <div className="flex flex-wrap gap-2">
                              <Button
                                size="sm"
                                variant="outline"
                                onClick={() => void loadFeishuMultiAgentBindings()}
                                disabled={routesLoading || routesSaving || existingBinding || bindingPhase === "finalizing"}
                              >
                                {routesLoading ? <Loader2 className="animate-spin" /> : <RefreshCw size={14} />}
                                重新读取
                              </Button>
                              <Button
                                size="sm"
                                onClick={handleAddFeishuRoute}
                                disabled={routesLoading || routesSaving || routeAgentOptions.length === 0}
                              >
                                <Plus size={14} />
                                新增规则
                              </Button>
                            </div>
                          </div>

                          {(routesError || routesHint) && (
                            <div className={`rounded-lg px-3 py-2.5 text-[12px] ${routesError
                              ? "border border-red-500/20 bg-red-500/8 text-red-300"
                              : "border border-emerald-500/15 bg-emerald-500/8 text-emerald-100/85"}`}
                            >
                              {routesError || routesHint}
                            </div>
                          )}

                          <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                            <p>优先级从高到低是：精确私聊/群组 ID、账号级路由、全部账号回退、默认 Agent。</p>
                            <p className="mt-1">如果两条规则属于同一优先级，OpenClaw 会按这里的顺序命中；可以用上下箭头调整。</p>
                          </div>

                          {routesLoading ? (
                            <div className="flex items-center justify-center py-10 text-muted-foreground">
                              <Loader2 size={16} className="mr-2 animate-spin" />
                              <span className="text-[12px]">正在读取飞书多 Agent 路由...</span>
                            </div>
                          ) : routeAgentOptions.length === 0 ? (
                            <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-4 text-[12px] text-amber-100/85">
                              还没有可选的 Agent。请先在 Agents 页面创建至少一个 Agent，再回来设置飞书路由。
                            </div>
                          ) : feishuRoutes.length === 0 ? (
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-4 text-[12px] text-muted-foreground">
                              当前还没有飞书多 Agent 路由。你可以先加一条账号级规则，或者直接按用户 Open ID / 群组 ID 做精确分流。
                            </div>
                          ) : (
                            <div className="space-y-3">
                              {feishuRoutes.map((route, index) => {
                                const resolvedAccountValue = route.accountId?.trim() ? route.accountId.trim() : FEISHU_DEFAULT_ACCOUNT_SCOPE;
                                const peerPlaceholder = route.scope === "group" ? "oc_xxx 群组 ID" : "ou_xxx 用户 Open ID";

                                return (
                                  <Card
                                    key={`${index}-${route.agentId}-${route.scope}-${route.accountId ?? ""}-${route.peerId ?? ""}`}
                                    className="border-white/[0.06] bg-white/[0.03]"
                                  >
                                    <CardContent className="space-y-4 p-4">
                                      <div className="flex flex-col gap-2 lg:flex-row lg:items-center lg:justify-between">
                                        <div className="flex flex-wrap items-center gap-2">
                                          <Badge variant="secondary" className="border-0 bg-sky-500/10 text-sky-200">
                                            规则 {index + 1}
                                          </Badge>
                                          <span className="text-[11px] text-muted-foreground">
                                            {route.scope === "account" ? "账号级路由" : route.scope === "group" ? "群组精确路由" : "私聊精确路由"}
                                          </span>
                                        </div>

                                        <div className="flex items-center gap-1">
                                          <Button
                                            size="icon"
                                            variant="ghost"
                                            className="h-7 w-7"
                                            onClick={() => handleMoveFeishuRoute(index, -1)}
                                            disabled={routesSaving || index === 0}
                                          >
                                            <ChevronUp size={13} />
                                          </Button>
                                          <Button
                                            size="icon"
                                            variant="ghost"
                                            className="h-7 w-7"
                                            onClick={() => handleMoveFeishuRoute(index, 1)}
                                            disabled={routesSaving || index === feishuRoutes.length - 1}
                                          >
                                            <ChevronDown size={13} />
                                          </Button>
                                          <Button
                                            size="icon"
                                            variant="ghost"
                                            className="h-7 w-7 text-muted-foreground hover:text-red-400"
                                            onClick={() => handleRemoveFeishuRoute(index)}
                                            disabled={routesSaving}
                                          >
                                            <Trash2 size={13} />
                                          </Button>
                                        </div>
                                      </div>

                                      <div className="grid gap-3 xl:grid-cols-2">
                                        <div className="space-y-1">
                                          <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">Agent</label>
                                          <div className="relative">
                                            <Bot size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
                                            <select
                                              className={`${inputCls} appearance-none pl-9`}
                                              value={route.agentId}
                                              onChange={(event) => handleUpdateFeishuRoute(index, { agentId: event.target.value })}
                                              disabled={routesSaving}
                                            >
                                              <option value="">选择 Agent</option>
                                              {routeAgentOptions.map((agent) => (
                                                <option key={agent.id} value={agent.id}>{agent.label}</option>
                                              ))}
                                            </select>
                                          </div>
                                        </div>

                                        <div className="space-y-1">
                                          <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">账号范围</label>
                                          <select
                                            className={`${inputCls} appearance-none`}
                                            value={resolvedAccountValue}
                                            onChange={(event) => handleUpdateFeishuRoute(index, {
                                              accountId: event.target.value === FEISHU_DEFAULT_ACCOUNT_SCOPE ? null : event.target.value,
                                            })}
                                            disabled={routesSaving}
                                          >
                                            <option value={FEISHU_DEFAULT_ACCOUNT_SCOPE}>默认账号 ({feishuDefaultAccountId})</option>
                                            <option value="*">全部账号</option>
                                            {routeAccountOptions.map((account) => (
                                              <option key={account.id} value={account.id}>{account.label}</option>
                                            ))}
                                          </select>
                                        </div>
                                      </div>

                                      <div className="space-y-2">
                                        <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">匹配方式</label>
                                        <div className="grid grid-cols-3 gap-2">
                                          {([
                                            { value: "account", label: "账号" },
                                            { value: "direct", label: "私聊" },
                                            { value: "group", label: "群组" },
                                          ] as Array<{ value: FeishuRouteScope; label: string }>).map((item) => (
                                            <Button
                                              key={item.value}
                                              type="button"
                                              size="sm"
                                              variant="outline"
                                              className={route.scope === item.value
                                                ? "border-sky-400/40 bg-sky-500/15 text-sky-50 hover:bg-sky-500/20 hover:text-sky-50"
                                                : "border-white/[0.08] bg-white/[0.02] text-muted-foreground hover:bg-white/[0.04]"}
                                              onClick={() => handleUpdateFeishuRoute(index, { scope: item.value })}
                                              disabled={routesSaving}
                                            >
                                              {item.label}
                                            </Button>
                                          ))}
                                        </div>
                                      </div>

                                      {route.scope !== "account" && (
                                        <div className="space-y-1">
                                          <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">
                                            {route.scope === "group" ? "群组 ID" : "用户 Open ID"}
                                          </label>
                                          <input
                                            className={inputCls}
                                            placeholder={peerPlaceholder}
                                            value={route.peerId ?? ""}
                                            onChange={(event) => handleUpdateFeishuRoute(index, { peerId: event.target.value })}
                                            disabled={routesSaving}
                                            autoCapitalize="off"
                                            autoComplete="off"
                                            spellCheck={false}
                                          />
                                          <p className="text-[11px] text-muted-foreground">
                                            {route.scope === "group" ? "飞书群组 ID 一般形如 `oc_xxx`。" : "飞书用户 Open ID 一般形如 `ou_xxx`。"}
                                          </p>
                                        </div>
                                      )}
                                    </CardContent>
                                  </Card>
                                );
                              })}
                            </div>
                          )}

                          <div className="flex flex-wrap gap-2">
                            <Button
                              size="sm"
                              onClick={() => void handleSaveFeishuRoutes()}
                              disabled={routesSaving || routesLoading || routeAgentOptions.length === 0}
                            >
                              {routesSaving ? <Loader2 className="animate-spin" /> : <CheckCircle2 size={14} />}
                              {routesSaving ? "保存中..." : "保存多 Agent 路由"}
                            </Button>
                            <Button
                              size="sm"
                              variant="outline"
                              onClick={() => {
                                setRoutesError("");
                                setRoutesHint("");
                                setFeishuRoutes([]);
                              }}
                              disabled={routesSaving || routesLoading || feishuRoutes.length === 0}
                            >
                              清空当前列表
                            </Button>
                          </div>
                        </CardContent>
                      </Card>
                    </div>
                  )}
                </>
              )}
            </CardContent>
          </Card>
        </div>
      )}
    </TooltipProvider>
  );
}
