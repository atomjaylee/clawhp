import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import QRCode from "qrcode";
import {
  Bot,
  CheckCircle2,
  Loader2,
  MessageSquare,
  Plus,
  Radio,
  RefreshCw,
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
import ModuleTabs, { type ModuleTabItem } from "@/components/ui/module-tabs";
import PageShell from "@/components/PageShell";
import type { AgentInfo, CommandResult, LogEntry } from "@/types";

interface ChannelEntry {
  channel: string;
  account: string;
  name: string;
  enabled: boolean;
  boundAgentId?: string | null;
  boundAgentName?: string | null;
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
type FeishuDialogTab = "install" | "agent" | "qr" | "existing" | "status";
type ChannelsModuleTab = "list" | "guide";

interface ExistingFeishuBindingForm {
  appId: string;
  appSecret: string;
  domain: "feishu" | "lark";
}

interface PendingRemoval {
  channel: string;
  account: string;
  label: string;
}

interface FeishuAccountBindingSummary {
  accountId: string;
  displayName: string;
  appId: string;
  domain: string;
  enabled: boolean;
  boundAgentId?: string | null;
}

interface FeishuAccountBindingCatalog {
  defaultAccountId: string;
  accounts: FeishuAccountBindingSummary[];
}

interface FeishuChannelConfig {
  accountId: string;
  displayName: string;
  appId: string;
  appSecret: string;
  domain: string;
  enabled: boolean;
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

function getChannelInfo(ch: string) {
  return CHANNEL_LABELS[ch] ?? { label: ch, color: "bg-white/10 text-foreground/70" };
}

function findFeishuAccountSummary(
  catalog: FeishuAccountBindingCatalog | null,
  accountId: string | null,
) {
  const resolvedAccountId = accountId?.trim();
  if (!resolvedAccountId) {
    return null;
  }

  return catalog?.accounts.find((account) => account.accountId === resolvedAccountId) ?? null;
}

function resolveFeishuAgentSelection(
  agents: AgentInfo[],
  catalog: FeishuAccountBindingCatalog | null,
  accountId: string | null,
  currentSelectedAgentId: string,
) {
  const resolvedAccountId = accountId?.trim() ?? "";
  const currentAccount = findFeishuAccountSummary(catalog, resolvedAccountId);
  const currentBoundAgentId = currentAccount?.boundAgentId?.trim() ?? "";

  if (currentBoundAgentId) {
    return currentBoundAgentId;
  }

  const boundElsewhere = new Set(
    (catalog?.accounts ?? [])
      .filter((account) => account.accountId !== resolvedAccountId)
      .map((account) => account.boundAgentId?.trim() ?? "")
      .filter(Boolean),
  );

  const selectedAgentId = currentSelectedAgentId.trim();
  if (
    selectedAgentId
    && !boundElsewhere.has(selectedAgentId)
    && agents.some((agent) => agent.id === selectedAgentId)
  ) {
    return selectedAgentId;
  }

  return agents.find((agent) => !boundElsewhere.has(agent.id))?.id ?? "";
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
        boundAgentId: typeof account.boundAgentId === "string" ? account.boundAgentId : null,
        boundAgentName: typeof account.boundAgentName === "string" ? account.boundAgentName : null,
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
  const [loading, setLoading] = useState(true);
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [pendingRemoval, setPendingRemoval] = useState<PendingRemoval | null>(null);
  const [removeError, setRemoveError] = useState("");

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
  const [feishuBindingCatalog, setFeishuBindingCatalog] = useState<FeishuAccountBindingCatalog | null>(null);
  const [bindingCatalogLoading, setBindingCatalogLoading] = useState(false);
  const [bindingCatalogError, setBindingCatalogError] = useState("");
  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [moduleTab, setModuleTab] = useState<ChannelsModuleTab>("list");
  const [dialogTab, setDialogTab] = useState<FeishuDialogTab>("install");
  const [unbindLoading, setUnbindLoading] = useState(false);

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

  const agentLabelById = useMemo(() => {
    const labels = new Map<string, string>();

    for (const agent of availableAgents) {
      labels.set(agent.id, agent.name || agent.id);
    }

    for (const account of feishuBindingCatalog?.accounts ?? []) {
      const boundAgentId = account.boundAgentId?.trim();
      if (boundAgentId && !labels.has(boundAgentId)) {
        labels.set(boundAgentId, `${boundAgentId}（已缺失）`);
      }
    }

    return labels;
  }, [availableAgents, feishuBindingCatalog]);

  const editingFeishuAccount = useMemo(
    () => findFeishuAccountSummary(feishuBindingCatalog, editingAccountId),
    [editingAccountId, feishuBindingCatalog],
  );

  const selectedAgentOptions = useMemo(() => {
    const options = new Map<string, string>();
    const currentAccountId = editingAccountId?.trim() ?? "";
    const lockedAgentId = editingFeishuAccount?.boundAgentId?.trim() ?? "";
    const boundElsewhere = new Set(
      (feishuBindingCatalog?.accounts ?? [])
        .filter((account) => account.accountId !== currentAccountId)
        .map((account) => account.boundAgentId?.trim() ?? "")
        .filter(Boolean),
    );

    for (const agent of availableAgents) {
      if (!boundElsewhere.has(agent.id) || agent.id === lockedAgentId) {
        options.set(agent.id, agent.name || agent.id);
      }
    }

    const fallbackAgentId = lockedAgentId || selectedAgentId.trim();
    if (fallbackAgentId && !options.has(fallbackAgentId) && !boundElsewhere.has(fallbackAgentId)) {
      options.set(fallbackAgentId, agentLabelById.get(fallbackAgentId) || `${fallbackAgentId}（已缺失）`);
    }

    return Array.from(options.entries()).map(([id, label]) => ({ id, label }));
  }, [agentLabelById, availableAgents, editingAccountId, editingFeishuAccount, feishuBindingCatalog, selectedAgentId]);

  const currentBoundAgentId = editingFeishuAccount?.boundAgentId?.trim() ?? "";
  const currentBoundAgentLabel = currentBoundAgentId
    ? (agentLabelById.get(currentBoundAgentId) || currentBoundAgentId)
    : "";
  const dialogBusy = installPhase === "running"
    || bindingPhase === "waiting"
    || bindingPhase === "finalizing"
    || existingBinding
    || unbindLoading;
  const bindingStatusLabel = unbindLoading
    ? "解绑中"
    : existingBinding
      ? "校验已有机器人"
      : bindingPhase === "done"
        ? "已完成"
        : bindingPhase === "finalizing"
          ? "写入配置中"
          : bindingPhase === "waiting"
            ? "等待扫码"
            : currentBoundAgentId
              ? "已绑定"
              : "尚未开始";
  const dialogTabs: ModuleTabItem<FeishuDialogTab>[] = setupStep === "install"
    ? [
      {
        id: "install",
        label: "安装插件",
        icon: Settings2,
        badge: installPhase === "running" ? "进行中" : "需要",
      },
      {
        id: "status",
        label: "当前状态",
        icon: CheckCircle2,
        badge: feishuStatus?.officialPluginInstalled ? "已装" : "待装",
      },
    ]
    : [
      {
        id: "agent",
        label: "绑定关系",
        icon: Bot,
        badge: currentBoundAgentId ? "已绑定" : (selectedAgentId ? "已选" : "待选"),
      },
      {
        id: "qr",
        label: "扫码创建",
        icon: Radio,
        badge: bindingPhase === "waiting" ? "进行中" : undefined,
      },
      {
        id: "existing",
        label: "已有机器人",
        icon: MessageSquare,
        badge: existingBinding ? "提交中" : (existingBindingForm.appId.trim() ? "已填" : undefined),
      },
      {
        id: "status",
        label: "当前状态",
        icon: CheckCircle2,
        badge: currentBoundAgentId ? "已绑定" : (setupStep === "done" ? "完成" : "待绑定"),
      },
    ];

  const loadFeishuBindingCatalog = useCallback(async (targetAccountId?: string | null) => {
    setBindingCatalogLoading(true);
    setBindingCatalogError("");

    try {
      const [catalogResult, agentList] = await Promise.all([
        invoke("get_feishu_channel_binding_catalog") as Promise<CommandResult>,
        invoke("list_agents") as Promise<AgentInfo[]>,
      ]);
      const agents = Array.isArray(agentList) ? agentList : [];
      setAvailableAgents(agents);

      if (!catalogResult.success) {
        setBindingCatalogError(catalogResult.stderr || "飞书绑定关系读取失败");
        setFeishuBindingCatalog(null);
        setSelectedAgentId("");
        return null;
      }

      const catalog = parseJsonValue<FeishuAccountBindingCatalog | null>(catalogResult.stdout, null);
      if (!catalog) {
        setBindingCatalogError("飞书绑定关系解析失败");
        setFeishuBindingCatalog(null);
        setSelectedAgentId("");
        return null;
      }

      setFeishuBindingCatalog(catalog);
      setSelectedAgentId((prev) => resolveFeishuAgentSelection(
        agents,
        catalog,
        targetAccountId ?? editingAccountId,
        prev,
      ));
      return catalog;
    } catch (e) {
      setBindingCatalogError(`${e}`);
      setAvailableAgents([]);
      setFeishuBindingCatalog(null);
      setSelectedAgentId("");
      return null;
    } finally {
      setBindingCatalogLoading(false);
    }
  }, [editingAccountId]);

  const loadFeishuChannelConfig = useCallback(async (accountId?: string | null) => {
    const resolvedAccountId = accountId?.trim();
    if (!resolvedAccountId) {
      setExistingBindingForm({
        appId: "",
        appSecret: "",
        domain: "feishu",
      });
      return null;
    }

    try {
      const result: CommandResult = await invoke("get_feishu_channel_config", {
        accountId: resolvedAccountId,
      });
      if (!result.success) {
        setSetupError(result.stderr || "飞书频道配置读取失败");
        return null;
      }

      const parsed = parseJsonValue<FeishuChannelConfig | null>(result.stdout, null);
      if (!parsed) {
        setSetupError("飞书频道配置解析失败");
        return null;
      }

      setExistingBindingForm({
        appId: parsed.appId || "",
        appSecret: "",
        domain: parsed.domain === "lark" ? "lark" : "feishu",
      });
      return parsed;
    } catch (e) {
      setSetupError(`${e}`);
      return null;
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

  const refreshAll = useCallback(async (options?: { silent?: boolean }) => {
    await fetchChannels({ silent: options?.silent });
  }, [fetchChannels]);

  const refreshFeishuDisplayNames = useCallback(async (accountId?: string | null) => {
    try {
      const result: CommandResult = await invoke("refresh_feishu_channel_display_names", {
        accountId: accountId?.trim() || null,
      });
      if (!result.success) {
        return false;
      }

      if (/已刷新\s+\d+\s+个飞书频道名称/.test(result.stdout)) {
        await refreshAll({ silent: true });
        return true;
      }
    } catch {
      // Best effort only.
    }

    return false;
  }, [refreshAll]);

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
          const nextChannels = extractChannelEntries(data);
          setChannels(nextChannels);
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
    };

    void bootstrap();

    return () => {
      cancelled = true;
    };
  }, []);

  const statuses = useMemo(() => {
    return channels.map((channel) => {
      if (!channel.enabled) {
        return {
          channel: channel.channel,
          account: channel.account,
          state: "disabled" as const,
          message: "已禁用",
        };
      }

      if (channel.channel === "feishu") {
        if (!channel.boundAgentId) {
          return {
            channel: channel.channel,
            account: channel.account,
            state: "issue" as const,
            message: "尚未绑定 Agent",
          };
        }

        return {
          channel: channel.channel,
          account: channel.account,
          state: "configured" as const,
          message: "已绑定 Agent，等待消息接入",
        };
      }

      return {
        channel: channel.channel,
        account: channel.account,
        state: "configured" as const,
        message: "已配置",
      };
    });
  }, [channels]);

  const getStatus = useCallback((channel: string, account: string) => (
    statuses.find((entry) => entry.channel === channel && (entry.account === account || entry.account === "default"))
  ), [statuses]);

  const resetFeishuDialogState = useCallback(() => {
    setSetupError("");
    setSetupStep("install");
    setDialogTab("install");
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
    setFeishuBindingCatalog(null);
    setBindingCatalogLoading(false);
    setBindingCatalogError("");
    setSelectedAgentId("");
    setUnbindLoading(false);
  }, []);

  useEffect(() => {
    if (!dialogOpen) {
      return;
    }

    setDialogTab((current) => {
      if (setupStep === "install") {
        return current === "status" ? "status" : "install";
      }

      return current === "install" ? "agent" : current;
    });
  }, [dialogOpen, setupStep]);

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
      return parsed;
    } catch (e) {
      setSetupError(`${e}`);
      return null;
    } finally {
      setSetupLoading(false);
    }
  }, []);

  const applyFeishuBindingSuccess = useCallback((message: string, next: { accountId: string; appId: string; domain: string }) => {
    const resolvedDomain = next.domain === "lark" ? "lark" : "feishu";

    setSetupError("");
    setSetupStep("done");
    setBindingPhase("done");
    setBindingError("");
    setExistingBindingError("");
    setBindingHint(message);
    setAuthSession(null);
    setAuthQrDataUrl("");
    setEditingAccountId(next.accountId);
    setFeishuStatus((prev) => ({
      officialPluginInstalled: true,
      officialPluginEnabled: true,
      communityPluginEnabled: prev?.communityPluginEnabled ?? false,
      channelConfigured: true,
      appId: next.appId || prev?.appId || "",
      displayName: prev?.displayName || next.accountId || "飞书官方插件",
      domain: resolvedDomain,
    }));
    setExistingBindingForm((prev) => ({
      appId: next.appId || prev.appId,
      appSecret: "",
      domain: resolvedDomain,
    }));

    void (async () => {
      try {
        await Promise.all([
          loadFeishuSetup(),
          loadFeishuBindingCatalog(next.accountId),
          loadFeishuChannelConfig(next.accountId),
          refreshAll({ silent: true }),
          refreshFeishuDisplayNames(next.accountId),
        ]);
      } catch {
        // Keep the optimistic UI if background status sync fails.
      }
    })();
  }, [loadFeishuBindingCatalog, loadFeishuChannelConfig, loadFeishuSetup, refreshAll, refreshFeishuDisplayNames]);

  const beginFeishuBinding = useCallback(async () => {
    setSetupError("");
    setBindingError("");
    setExistingBindingError("");
    setBindingHint("");
    setBindingPhase("idle");
    setAuthSession(null);
    setAuthQrDataUrl("");

    if (currentBoundAgentId) {
      setBindingError("当前飞书频道已绑定 Agent，请先解绑后再重新绑定。");
      return;
    }
    if (!selectedAgentId.trim()) {
      setBindingError("请先选择一个未绑定的 Agent。");
      return;
    }

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
  }, [currentBoundAgentId, selectedAgentId]);

  const openFeishuDialog = useCallback(async (accountId?: string) => {
    const targetAccountId = accountId?.trim() ? accountId.trim() : null;
    setEditingAccountId(targetAccountId);
    setDialogOpen(true);
    resetFeishuDialogState();
    await Promise.all([
      loadFeishuSetup(),
      loadFeishuBindingCatalog(targetAccountId),
      loadFeishuChannelConfig(targetAccountId),
    ]);
    void refreshFeishuDisplayNames(targetAccountId);
  }, [loadFeishuBindingCatalog, loadFeishuChannelConfig, loadFeishuSetup, refreshFeishuDisplayNames, resetFeishuDialogState]);

  const closeDialog = useCallback(() => {
    if (dialogBusy) {
      return;
    }
    setDialogOpen(false);
    setEditingAccountId(null);
    resetFeishuDialogState();
  }, [dialogBusy, resetFeishuDialogState]);

  const openRemoveDialog = useCallback((channel: string, account: string) => {
    setRemoveError("");
    setPendingRemoval({
      channel,
      account,
      label: getChannelInfo(channel).label,
    });
  }, []);

  const closeRemoveDialog = useCallback(() => {
    if (removing) {
      return;
    }
    setPendingRemoval(null);
    setRemoveError("");
  }, [removing]);

  const handleRemove = useCallback(async () => {
    if (!pendingRemoval) {
      return;
    }

    const { channel, account } = pendingRemoval;
    const key = `${channel}:${account}`;
    setRemoveError("");
    setRemoving(key);

    try {
      const result: CommandResult = await invoke("remove_channel", { channel, account });
      if (!result.success) {
        setRemoveError(result.stderr || "移除频道失败");
        return;
      }

      setChannels((prev) => prev.filter((entry) => !(entry.channel === channel && entry.account === account)));
      setPendingRemoval(null);
      void refreshAll({ silent: true });
    } catch (e) {
      setRemoveError(`移除频道失败: ${e}`);
    } finally {
      setRemoving(null);
    }
  }, [pendingRemoval, refreshAll]);

  const handleUnbindFeishu = useCallback(async () => {
    const accountId = editingAccountId?.trim();
    if (!accountId) {
      return;
    }

    setSetupError("");
    setBindingError("");
    setExistingBindingError("");
    setBindingHint("");
    setUnbindLoading(true);
    setAuthSession(null);
    setAuthQrDataUrl("");
    setBindingPhase("idle");

    try {
      const result: CommandResult = await invoke("unbind_feishu_channel_account", {
        accountId,
      });
      if (!result.success) {
        setExistingBindingError(result.stderr || "飞书频道解绑失败");
        return;
      }

      setSetupStep("bind");
      setBindingHint(result.stdout || "飞书频道已解绑，现在可以重新选择 Agent。");
      await Promise.all([
        loadFeishuSetup(),
        loadFeishuBindingCatalog(accountId),
        loadFeishuChannelConfig(accountId),
        refreshAll({ silent: true }),
      ]);
    } catch (e) {
      setExistingBindingError(`${e}`);
    } finally {
      setUnbindLoading(false);
    }
  }, [editingAccountId, loadFeishuBindingCatalog, loadFeishuChannelConfig, loadFeishuSetup, refreshAll]);

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
    const accountId = editingAccountId?.trim() || appId;

    if (!appId) {
      setExistingBindingError("请先填写飞书 App ID");
      return;
    }
    if (!appSecret) {
      setExistingBindingError("请先填写飞书 App Secret");
      return;
    }
    if (currentBoundAgentId) {
      setExistingBindingError("当前飞书频道已绑定 Agent，请先解绑后再重新绑定。");
      return;
    }
    if (!selectedAgentId.trim()) {
      setExistingBindingError("请先选择一个未绑定的 Agent。");
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
        accountId,
        displayName: editingFeishuAccount?.displayName || null,
        agentId: selectedAgentId,
      });

      if (!result.success) {
        setExistingBindingError(result.stderr || "已有飞书机器人绑定失败");
        return;
      }

      applyFeishuBindingSuccess(
        result.stdout || "已有飞书机器人绑定完成，可以回到频道列表继续使用。",
        {
          accountId,
          appId,
          domain: existingBindingForm.domain,
        },
      );
    } catch (e) {
      setExistingBindingError(`${e}`);
    } finally {
      setExistingBinding(false);
    }
  }, [applyFeishuBindingSuccess, currentBoundAgentId, editingAccountId, editingFeishuAccount, existingBindingForm, selectedAgentId]);

  useEffect(() => {
    if (!authSession) {
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
          setAuthSession(null);
          setAuthQrDataUrl("");
          setBindingError(result.stderr || "飞书扫码状态获取失败");
          return;
        }

        const payload = parseJsonValue<FeishuAuthPollPayload | null>(result.stdout, null);
        if (!payload) {
          setBindingPhase("idle");
          setAuthSession(null);
          setAuthQrDataUrl("");
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
          const accountId = editingAccountId?.trim() || payload.appId;
          setBindingPhase("finalizing");
          setBindingHint("扫码成功，正在应用绑定结果...");

          const bindingResult: CommandResult = await invoke("complete_feishu_plugin_binding", {
            appId: payload.appId,
            appSecret: payload.appSecret,
            domain: payload.suggestedDomain ?? authSession.domain,
            openId: payload.openId ?? null,
            accountId,
            displayName: editingFeishuAccount?.displayName || null,
            agentId: selectedAgentId,
          });
          if (cancelled) {
            return;
          }

          if (!bindingResult.success) {
            setBindingPhase("idle");
            setAuthSession(null);
            setAuthQrDataUrl("");
            setBindingError(bindingResult.stderr || "飞书绑定失败");
            return;
          }

          applyFeishuBindingSuccess(
            bindingResult.stdout || "飞书已绑定完成，可以回到频道列表继续使用。",
            {
              accountId,
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
        setAuthSession(null);
        setAuthQrDataUrl("");
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
          setAuthSession(null);
          setAuthQrDataUrl("");
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
  }, [applyFeishuBindingSuccess, authSession, editingAccountId, editingFeishuAccount, selectedAgentId]);

  const showInstallLogs = installPhase === "running" || installLogs.length > 0;
  const moduleTabs: ModuleTabItem<ChannelsModuleTab>[] = [
    { id: "list", label: "已接入", icon: MessageSquare, badge: channels.length },
    { id: "guide", label: "飞书接入", icon: Settings2 },
  ];

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
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7"
                    onClick={() => void refreshAll()}
                    disabled={loading}
                  >
                    {loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
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
        <ModuleTabs items={moduleTabs} value={moduleTab} onValueChange={setModuleTab} />

        {moduleTab === "list" && (
          <>
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
                              </div>
                              <p className="mt-0.5 text-[11px] text-muted-foreground">
                                账号: {channel.account}
                                {!channel.enabled && <span className="ml-2 text-amber-400">已禁用</span>}
                              </p>
                              {channel.channel === "feishu" && (
                                <p className="mt-1 text-[11px] text-muted-foreground">
                                  Agent: {channel.boundAgentName || channel.boundAgentId || "未绑定"}
                                </p>
                              )}
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
                                  onClick={() => openRemoveDialog(channel.channel, channel.account)}
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
          </>
        )}

        {moduleTab === "guide" && (
          <div className="space-y-4">
            <Card className="border-white/[0.08] bg-[radial-gradient(circle_at_top_left,rgba(124,58,237,0.16),transparent_38%),linear-gradient(180deg,rgba(255,255,255,0.04),rgba(255,255,255,0.02))]">
              <CardContent className="space-y-4 p-5">
                <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                  <div>
                    <h3 className="text-[14px] font-semibold">飞书接入向导</h3>
                    <p className="mt-1 text-[12px] text-muted-foreground">
                      把频道管理拆成“已接入”和“接入向导”两个模块，平时先看列表，需要新增时再按步骤操作。
                    </p>
                  </div>
                  <Button size="sm" onClick={() => void openFeishuDialog()}>
                    <Plus size={14} />
                    添加飞书
                  </Button>
                </div>

                <div className="grid gap-3 md:grid-cols-3">
                  <Card className="border-white/[0.08] bg-black/10">
                    <CardContent className="space-y-2 p-4">
                      <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 1 步</p>
                      <h4 className="text-[13px] font-semibold">安装官方插件</h4>
                      <p className="text-[12px] leading-5 text-muted-foreground">
                        应用内自动检测飞书官方插件，缺失时直接安装，不再跳出终端窗口。
                      </p>
                    </CardContent>
                  </Card>
                  <Card className="border-white/[0.08] bg-black/10">
                    <CardContent className="space-y-2 p-4">
                      <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 2 步</p>
                      <h4 className="text-[13px] font-semibold">绑定 Agent</h4>
                      <p className="text-[12px] leading-5 text-muted-foreground">
                        频道和 Agent 保持一对一关系，扫码创建或绑定已有机器人时一起完成路由。
                      </p>
                    </CardContent>
                  </Card>
                  <Card className="border-white/[0.08] bg-black/10">
                    <CardContent className="space-y-2 p-4">
                      <p className="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">第 3 步</p>
                      <h4 className="text-[13px] font-semibold">后台生效</h4>
                      <p className="text-[12px] leading-5 text-muted-foreground">
                        绑定完成后会自动刷新频道列表和后台配置，列表模块里会立即看到结果。
                      </p>
                    </CardContent>
                  </Card>
                </div>
              </CardContent>
            </Card>
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
                        弹窗里只保留实际操作和实时状态，外层步骤说明不再重复展示。
                      </p>
                    </div>
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={closeDialog} disabled={dialogBusy}>
                  关闭
                </Button>
              </div>

              {setupLoading || bindingCatalogLoading ? (
                <div className="flex items-center justify-center py-16 text-muted-foreground">
                  <Loader2 size={18} className="mr-2 animate-spin" />
                  <span className="text-[13px]">正在检查飞书配置...</span>
                </div>
              ) : (
                <>
                  {(setupError || bindingError || existingBindingError || bindingCatalogError) && (
                    <div className="rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                      {setupError || bindingError || existingBindingError || bindingCatalogError}
                    </div>
                  )}
                  {bindingHint && (
                    <div className="rounded-lg border border-emerald-500/15 bg-emerald-500/8 px-3 py-2.5 text-[12px] text-emerald-100/85">
                      {bindingHint}
                    </div>
                  )}
                  <ModuleTabs items={dialogTabs} value={dialogTab} onValueChange={setDialogTab} />

                  {setupStep === "install" && dialogTab === "install" && (
                    <Card className="border-white/[0.08] bg-white/[0.02]">
                      <CardContent className="space-y-4 p-4">
                        <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                          <div className="space-y-1">
                            <h4 className="text-[14px] font-semibold">官方插件安装</h4>
                            <p className="text-[12px] text-muted-foreground">
                              当前环境还没装好官方飞书插件。安装完成后，这个弹窗里的绑定标签页就可以直接继续使用。
                            </p>
                          </div>
                          <Badge variant="outline" className="h-6 border-white/[0.08] bg-white/[0.03] px-2 text-[10px] text-muted-foreground">
                            {installPhase === "running" ? "安装中" : installSuccess ? "已完成" : "待安装"}
                          </Badge>
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

                  {setupStep !== "install" && dialogTab === "agent" && (
                    <div className="grid gap-4 xl:grid-cols-[1.05fr_0.95fr]">
                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[14px] font-semibold">绑定关系</h4>
                            <p className="text-[12px] text-muted-foreground">
                              先把这个飞书频道对应到一个 Agent，后面的扫码创建和已有机器人绑定都会直接使用这里的选择。
                            </p>
                          </div>

                          <div className="space-y-1">
                            <label className="text-[11px] uppercase tracking-[0.12em] text-muted-foreground">目标 Agent</label>
                            <div className="relative">
                              <Bot size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
                              <select
                                className={`${inputCls} appearance-none pl-9`}
                                value={selectedAgentId}
                                onChange={(event) => setSelectedAgentId(event.target.value)}
                                disabled={Boolean(currentBoundAgentId) || existingBinding || unbindLoading || bindingPhase === "waiting" || bindingPhase === "finalizing" || installPhase === "running"}
                              >
                                <option value="">选择未绑定的 Agent</option>
                                {selectedAgentOptions.map((agent) => (
                                  <option key={agent.id} value={agent.id}>{agent.label}</option>
                                ))}
                              </select>
                            </div>
                          </div>

                          {currentBoundAgentId ? (
                            <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-3 text-[12px] text-amber-100/90">
                              当前频道已经绑定到 {currentBoundAgentLabel}。如果要改绑，请先解绑。
                            </div>
                          ) : selectedAgentId ? (
                            <div className="rounded-xl border border-emerald-500/15 bg-emerald-500/8 p-3 text-[12px] text-emerald-100/90">
                              当前会把这个飞书频道绑定到 {agentLabelById.get(selectedAgentId) || selectedAgentId}。
                            </div>
                          ) : selectedAgentOptions.length === 0 ? (
                            <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-3 text-[12px] text-amber-100/90">
                              现在没有可用的未绑定 Agent。请先在 Agents 页面创建 Agent，或者先解绑一个已占用的频道。
                            </div>
                          ) : (
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                              选好 Agent 后，再切到“扫码创建”或“已有机器人”继续。
                            </div>
                          )}

                          {editingAccountId && (
                            <div className="flex flex-wrap gap-2">
                              <Button
                                size="sm"
                                variant="outline"
                                onClick={() => void loadFeishuBindingCatalog(editingAccountId)}
                                disabled={bindingCatalogLoading || existingBinding || unbindLoading || bindingPhase === "finalizing"}
                              >
                                {bindingCatalogLoading ? <Loader2 className="animate-spin" /> : <RefreshCw size={14} />}
                                刷新绑定关系
                              </Button>
                              <Button
                                size="sm"
                                variant="outline"
                                onClick={() => void handleUnbindFeishu()}
                                disabled={!currentBoundAgentId || existingBinding || unbindLoading || bindingPhase === "finalizing"}
                              >
                                {unbindLoading ? <Loader2 className="animate-spin" /> : null}
                                {unbindLoading ? "解绑中..." : "解绑当前 Agent"}
                              </Button>
                            </div>
                          )}
                        </CardContent>
                      </Card>

                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[13px] font-semibold">当前频道</h4>
                            <p className="text-[12px] text-muted-foreground">这里集中看当前账号、Agent 和绑定状态。</p>
                          </div>

                          <div className="grid gap-3 sm:grid-cols-2 text-[12px]">
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">频道账号</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.displayName || editingAccountId || "新建频道"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">账号 ID</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.accountId || "待生成"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">当前 App ID</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.appId || existingBindingForm.appId || "尚未保存"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">绑定状态</p>
                              <p className="mt-1 font-medium text-foreground">{bindingStatusLabel}</p>
                            </div>
                          </div>

                          <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                            <p>当前 Agent：{currentBoundAgentLabel || (selectedAgentId ? (agentLabelById.get(selectedAgentId) || selectedAgentId) : "尚未选择")}</p>
                            <p className="mt-1">可选未绑定 Agent：{selectedAgentOptions.length}</p>
                          </div>

                          <div className="flex flex-wrap gap-2">
                            <Button size="sm" variant="outline" onClick={() => void refreshAll()} disabled={bindingPhase === "finalizing" || existingBinding || unbindLoading}>
                              <RefreshCw size={14} />
                              刷新频道列表
                            </Button>
                          </div>
                        </CardContent>
                      </Card>
                    </div>
                  )}

                  {setupStep !== "install" && dialogTab === "qr" && (
                    <div className="grid gap-4 lg:grid-cols-[1.1fr_0.9fr]">
                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[14px] font-semibold">扫码创建</h4>
                            <p className="text-[12px] text-muted-foreground">在这里生成二维码，用飞书扫一扫后自动创建并绑定到当前选中的 Agent。</p>
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
                              disabled={bindingPhase === "waiting" || bindingPhase === "finalizing" || existingBinding || unbindLoading || installPhase === "running" || !selectedAgentId.trim() || Boolean(currentBoundAgentId)}
                            >
                              {bindingPhase === "finalizing" ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                              {bindingPhase === "waiting"
                                ? "等待扫码中..."
                                : bindingPhase === "finalizing"
                                  ? "写入配置中..."
                                  : currentBoundAgentId
                                    ? "已绑定，请先解绑"
                                    : !selectedAgentId.trim()
                                      ? "先选择 Agent"
                                      : "开始扫码创建"}
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
                                disabled={existingBinding || unbindLoading}
                              >
                                取消扫码
                              </Button>
                            )}
                          </div>
                        </CardContent>
                      </Card>

                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[13px] font-semibold">当前上下文</h4>
                            <p className="text-[12px] text-muted-foreground">扫码前先确认目标频道和 Agent 没问题。</p>
                          </div>

                          <div className="space-y-3 text-[12px]">
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">目标 Agent</p>
                              <p className="mt-1 font-medium text-foreground">{selectedAgentId ? (agentLabelById.get(selectedAgentId) || selectedAgentId) : "尚未选择"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">当前频道</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.displayName || editingAccountId || "新建频道"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">扫码状态</p>
                              <p className="mt-1 font-medium text-foreground">{bindingStatusLabel}</p>
                            </div>
                          </div>

                          {currentBoundAgentId ? (
                            <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-3 text-[12px] text-amber-100/90">
                              当前频道已经绑定到 {currentBoundAgentLabel}。如果要重新扫码绑定，请先回到“绑定关系”里解绑。
                            </div>
                          ) : !selectedAgentId.trim() ? (
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                              还没选 Agent。先去“绑定关系”标签里选好目标 Agent，再回来扫码。
                            </div>
                          ) : null}
                        </CardContent>
                      </Card>
                    </div>
                  )}

                  {setupStep !== "install" && dialogTab === "existing" && (
                    <div className="grid gap-4 lg:grid-cols-[1.1fr_0.9fr]">
                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[14px] font-semibold">已有机器人</h4>
                            <p className="text-[12px] text-muted-foreground">如果你已经在飞书开放平台建好了机器人，这里直接填入凭证即可。</p>
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
                                  disabled={existingBinding || unbindLoading || bindingPhase === "finalizing"}
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
                                  disabled={existingBinding || unbindLoading || bindingPhase === "finalizing"}
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
                                disabled={existingBinding || unbindLoading || bindingPhase === "finalizing"}
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
                                disabled={existingBinding || unbindLoading || bindingPhase === "finalizing"}
                                autoCapitalize="off"
                                autoComplete="off"
                                spellCheck={false}
                              />
                            </div>
                          </div>

                          <div className="flex flex-wrap gap-2">
                            <Button
                              size="sm"
                              onClick={() => void handleBindExistingFeishu()}
                              disabled={existingBinding || unbindLoading || bindingPhase === "finalizing" || installPhase === "running" || !selectedAgentId.trim() || Boolean(currentBoundAgentId) || !existingBindingForm.appId.trim() || !existingBindingForm.appSecret.trim()}
                            >
                              {existingBinding ? <Loader2 className="animate-spin" /> : <CheckCircle2 size={14} />}
                              {existingBinding
                                ? "绑定中..."
                                : currentBoundAgentId
                                  ? "已绑定，请先解绑"
                                  : !selectedAgentId.trim()
                                    ? "先选择 Agent"
                                    : "保存并绑定"}
                            </Button>
                            <Button
                              size="sm"
                              variant="outline"
                              onClick={() => setExistingBindingForm((prev) => ({ ...prev, appSecret: "" }))}
                              disabled={existingBinding || unbindLoading || bindingPhase === "finalizing" || !existingBindingForm.appSecret}
                            >
                              清空密钥
                            </Button>
                          </div>
                        </CardContent>
                      </Card>

                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[13px] font-semibold">填写前确认</h4>
                            <p className="text-[12px] text-muted-foreground">这里保留和当前绑定最相关的上下文，避免来回翻看。</p>
                          </div>

                          <div className="space-y-3 text-[12px]">
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">目标 Agent</p>
                              <p className="mt-1 font-medium text-foreground">{selectedAgentId ? (agentLabelById.get(selectedAgentId) || selectedAgentId) : "尚未选择"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">当前频道</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.displayName || editingAccountId || "新建频道"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">账号 ID</p>
                              <p className="mt-1 font-medium text-foreground">{editingFeishuAccount?.accountId || "默认使用 App ID"}</p>
                            </div>
                          </div>

                          <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[12px] text-muted-foreground">
                            <p>新建频道默认会用 App ID 作为频道账号 ID；编辑已有频道时会保留原来的账号 ID。</p>
                            <p className="mt-1">如果当前频道已经绑定 Agent，需要先去“绑定关系”标签里解绑，才能重新写入新的机器人凭证。</p>
                          </div>
                        </CardContent>
                      </Card>
                    </div>
                  )}

                  {dialogTab === "status" && (
                    <div className="grid gap-4 lg:grid-cols-[1fr_0.95fr]">
                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[13px] font-semibold">当前状态</h4>
                            <p className="text-[12px] text-muted-foreground">这里汇总插件、频道和绑定进度，方便快速判断现在卡在哪一步。</p>
                          </div>

                          <div className="grid gap-3 sm:grid-cols-2 text-[12px]">
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">官方插件</p>
                              <p className="mt-1 font-medium text-foreground">{feishuStatus?.officialPluginInstalled ? "已安装" : "未安装"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">官方插件状态</p>
                              <p className="mt-1 font-medium text-foreground">{feishuStatus?.officialPluginEnabled ? "已启用" : "待启用"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">当前频道</p>
                              <p className="mt-1 font-medium text-foreground">{editingAccountId || "新建频道"}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">当前 Agent</p>
                              <p className="mt-1 font-medium text-foreground">
                                {currentBoundAgentLabel || (selectedAgentId ? (agentLabelById.get(selectedAgentId) || selectedAgentId) : "尚未选择")}
                              </p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">绑定状态</p>
                              <p className="mt-1 font-medium text-foreground">{bindingStatusLabel}</p>
                            </div>
                            <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                              <p className="text-muted-foreground">已接入飞书频道</p>
                              <p className="mt-1 font-medium text-foreground">{feishuBindingCatalog?.accounts.length ?? 0}</p>
                            </div>
                          </div>

                          {feishuStatus?.communityPluginEnabled && (
                            <div className="rounded-xl border border-amber-500/15 bg-amber-500/8 p-3 text-[12px] text-amber-100/85">
                              检测到旧的社区飞书插件仍配置为启用状态。应用内绑定会自动优先启用官方插件。
                            </div>
                          )}
                        </CardContent>
                      </Card>

                      <Card className="border-white/[0.08] bg-white/[0.02]">
                        <CardContent className="space-y-4 p-4">
                          <div className="space-y-1">
                            <h4 className="text-[13px] font-semibold">当前规则</h4>
                            <p className="text-[12px] text-muted-foreground">弹窗里只保留和单频道绑定最相关的规则，不再混入旧的多路由配置。</p>
                          </div>

                          <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-4 text-[12px] text-muted-foreground">
                            <p>1. 一个飞书频道只能绑定一个 Agent。</p>
                            <p className="mt-1">2. 一个 Agent 也只能绑定一个飞书频道。</p>
                            <p className="mt-1">3. 如果要换绑，先在“绑定关系”里解绑，再重新扫码或填写已有机器人。</p>
                          </div>

                          <div className="flex flex-wrap gap-2">
                            <Button size="sm" variant="outline" onClick={() => void refreshAll()} disabled={bindingPhase === "finalizing" || existingBinding || unbindLoading}>
                              <RefreshCw size={14} />
                              刷新频道列表
                            </Button>
                            {editingAccountId && (
                              <Button
                                size="sm"
                                variant="outline"
                                onClick={() => void loadFeishuBindingCatalog(editingAccountId)}
                                disabled={bindingCatalogLoading || existingBinding || unbindLoading || bindingPhase === "finalizing"}
                              >
                                {bindingCatalogLoading ? <Loader2 className="animate-spin" /> : <RefreshCw size={14} />}
                                刷新绑定关系
                              </Button>
                            )}
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

      {pendingRemoval && (
        <div className="fixed inset-0 z-[130] flex items-center justify-center bg-black/75 px-4 backdrop-blur-sm" onClick={closeRemoveDialog}>
          <Card className="w-full max-w-md border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40" onClick={(event) => event.stopPropagation()}>
            <CardContent className="space-y-4 p-5">
              <div className="space-y-1">
                <h3 className="text-[14px] font-semibold">确认删除频道</h3>
                <p className="text-[12px] text-muted-foreground">
                  确定要移除 {pendingRemoval.label} 频道 `{pendingRemoval.account}` 吗？这个操作会删除当前频道配置。
                </p>
                {pendingRemoval.channel === "feishu" && (
                  <p className="text-[12px] text-muted-foreground">
                    如果你只是想更换 Agent，建议先进入“管理飞书接入”里解绑，而不是直接删除频道。
                  </p>
                )}
              </div>

              {removeError && (
                <div className="rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                  {removeError}
                </div>
              )}

              <div className="flex justify-end gap-2">
                <Button size="sm" variant="outline" onClick={closeRemoveDialog} disabled={Boolean(removing)}>
                  取消
                </Button>
                <Button size="sm" onClick={() => void handleRemove()} disabled={Boolean(removing)}>
                  {removing ? <Loader2 className="animate-spin" /> : <Trash2 size={14} />}
                  {removing ? "删除中..." : "确认删除"}
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      )}
    </TooltipProvider>
  );
}
