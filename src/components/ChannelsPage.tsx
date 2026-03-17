import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ChevronDown,
  ExternalLink,
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
import type { CommandResult } from "@/types";

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

interface FeishuChannelForm {
  accountId: string;
  displayName: string;
  appId: string;
  appSecret: string;
  domain: "feishu" | "lark";
  connectionMode: "websocket" | "webhook";
  verificationToken: string;
  encryptKey: string;
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

const inputCls = "w-full h-9 px-3 text-[13px] rounded-lg border border-white/[0.08] bg-white/[0.03] text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-primary/50 focus:border-primary/30 transition-colors";

const emptyFeishuForm: FeishuChannelForm = {
  accountId: "default",
  displayName: "",
  appId: "",
  appSecret: "",
  domain: "feishu",
  connectionMode: "websocket",
  verificationToken: "",
  encryptKey: "",
};

const FEISHU_DOMAIN_OPTIONS = [
  { value: "feishu", label: "飞书（中国区）", description: "适合国内飞书租户" },
  { value: "lark", label: "Lark（国际版）", description: "适合海外或国际租户" },
] as const;

const FEISHU_CONNECTION_OPTIONS = [
  { value: "websocket", label: "WebSocket", description: "实时收消息，适合多数桌面场景" },
  { value: "webhook", label: "Webhook", description: "适合固定公网回调地址" },
] as const;

interface DialogSelectFieldProps {
  label: string;
  menuLabel: string;
  value: string;
  options: ReadonlyArray<{ value: string; label: string; description: string }>;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onValueChange: (value: string) => void;
  menuRef: RefObject<HTMLDivElement | null>;
}

function DialogSelectField({
  label,
  menuLabel,
  value,
  options,
  open,
  onOpenChange,
  onValueChange,
  menuRef,
}: DialogSelectFieldProps) {
  const selected = options.find((option) => option.value === value) ?? options[0];

  return (
    <div ref={menuRef} className="relative">
      <label className="mb-1.5 block text-[12px] text-muted-foreground">{label}</label>
      <button
        type="button"
        className="flex w-full items-center justify-between gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 text-left transition-colors hover:border-sky-500/25 hover:bg-white/[0.05] focus:outline-none focus:ring-1 focus:ring-primary/50"
        onClick={() => onOpenChange(!open)}
      >
        <div className="min-w-0">
          <p className="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
            当前选项
          </p>
          <p className="mt-1 truncate text-[12px] font-medium text-foreground/90">
            {selected.label}
          </p>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {selected.description}
          </p>
        </div>
        <ChevronDown
          size={15}
          className={`shrink-0 text-muted-foreground transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>

      {open && (
        <div className="absolute left-0 right-0 top-[calc(100%+8px)] z-30 overflow-hidden rounded-xl border border-white/[0.08] bg-[#10141b] shadow-2xl shadow-black/35">
          <div className="border-b border-white/[0.06] px-3 py-2 text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
            {menuLabel}
          </div>
          <div className="space-y-1 p-2">
            {options.map((option) => {
              const selectedOption = option.value === value;
              return (
                <button
                  key={option.value}
                  type="button"
                  className={`flex w-full items-start justify-between gap-3 rounded-lg px-3 py-2 text-left transition-colors ${
                    selectedOption ? "bg-sky-500/10 text-sky-100" : "hover:bg-white/[0.04]"
                  }`}
                  onClick={() => {
                    onValueChange(option.value);
                    onOpenChange(false);
                  }}
                >
                  <div className="min-w-0">
                    <p className="text-[12px] font-medium">{option.label}</p>
                    <p className="mt-1 text-[11px] text-muted-foreground">
                      {option.description}
                    </p>
                  </div>
                  <span className="pt-0.5">
                    {selectedOption ? <Check size={14} className="text-sky-300" /> : null}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function getChannelInfo(ch: string) {
  return CHANNEL_LABELS[ch] ?? { label: ch, color: "bg-white/10 text-foreground/70" };
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

function parseFeishuForm(raw: string): FeishuChannelForm | null {
  try {
    const data = parseJsonValue<Partial<FeishuChannelForm> | null>(raw, null);
    if (!data) {
      return null;
    }
    return {
      accountId: (data.accountId ?? "default").trim() || "default",
      displayName: data.displayName ?? "",
      appId: data.appId ?? "",
      appSecret: data.appSecret ?? "",
      domain: data.domain === "lark" ? "lark" : "feishu",
      connectionMode: data.connectionMode === "webhook" ? "webhook" : "websocket",
      verificationToken: data.verificationToken ?? "",
      encryptKey: data.encryptKey ?? "",
    };
  } catch {
    return null;
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

export default function ChannelsPage() {
  const [channels, setChannels] = useState<ChannelEntry[]>([]);
  const [statuses, setStatuses] = useState<ChannelStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [checkingStatus, setCheckingStatus] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState("");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingAccountId, setEditingAccountId] = useState<string | null>(null);
  const [formLoading, setFormLoading] = useState(false);
  const [formSaving, setFormSaving] = useState(false);
  const [formError, setFormError] = useState("");
  const [feishuForm, setFeishuForm] = useState<FeishuChannelForm>(emptyFeishuForm);
  const [activeDialogMenu, setActiveDialogMenu] = useState<"domain" | "connection" | null>(null);
  const domainMenuRef = useRef<HTMLDivElement | null>(null);
  const connectionMenuRef = useRef<HTMLDivElement | null>(null);

  const fetchChannels = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const result: CommandResult = await invoke("list_channels");
      if (!result.success) {
        setChannels([]);
        setError(result.stderr || "频道列表加载失败");
        return;
      }

      const data = result.stdout ? parseJsonValue<Record<string, unknown>>(result.stdout, {}) : {};
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
      setChannels(entries);
    } catch (e) {
      setChannels([]);
      setError(`${e}`);
    } finally {
      setLoading(false);
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

  useEffect(() => {
    void fetchChannels();
    void fetchStatus();
  }, [fetchChannels, fetchStatus]);

  useEffect(() => {
    if (!dialogOpen || !activeDialogMenu) {
      return undefined;
    }

    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (domainMenuRef.current?.contains(target) || connectionMenuRef.current?.contains(target)) {
        return;
      }
      setActiveDialogMenu(null);
    };

    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [activeDialogMenu, dialogOpen]);

  const refreshAll = useCallback(async () => {
    await Promise.all([fetchChannels(), fetchStatus()]);
  }, [fetchChannels, fetchStatus]);

  const getStatus = useCallback((channel: string, account: string) => (
    statuses.find((entry) => entry.channel === channel && (entry.account === account || entry.account === "default"))
  ), [statuses]);

  const loadFeishuForm = useCallback(async (accountId?: string) => {
    setFormLoading(true);
    setFormError("");
    try {
      const result: CommandResult = await invoke("get_feishu_channel_config", {
        accountId: accountId || null,
      });
      if (!result.success) {
        setFormError(result.stderr || "飞书配置加载失败");
        return;
      }
      const parsed = parseFeishuForm(result.stdout);
      setFeishuForm(parsed ?? { ...emptyFeishuForm, accountId: accountId || "default" });
    } catch (e) {
      setFormError(`${e}`);
    } finally {
      setFormLoading(false);
    }
  }, []);

  const openFeishuDialog = useCallback(async (accountId?: string) => {
    setEditingAccountId(accountId ?? null);
    setDialogOpen(true);
    setActiveDialogMenu(null);
    setFeishuForm({ ...emptyFeishuForm, accountId: accountId || "default" });
    await loadFeishuForm(accountId);
  }, [loadFeishuForm]);

  const closeDialog = () => {
    if (formSaving) return;
    setDialogOpen(false);
    setEditingAccountId(null);
    setFormError("");
    setActiveDialogMenu(null);
  };

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

  const canSaveFeishu = useMemo(() => {
    if (!feishuForm.accountId.trim() || !feishuForm.appId.trim() || !feishuForm.appSecret.trim()) {
      return false;
    }
    if (feishuForm.connectionMode === "webhook") {
      return Boolean(feishuForm.verificationToken.trim() && feishuForm.encryptKey.trim());
    }
    return true;
  }, [feishuForm]);

  const handleSaveFeishu = async () => {
    if (!canSaveFeishu) {
      setFormError("请先补全必填项");
      return;
    }

    setFormSaving(true);
    setFormError("");
    try {
      const result: CommandResult = await invoke("save_feishu_channel", {
        accountId: feishuForm.accountId.trim(),
        displayName: feishuForm.displayName.trim() || null,
        appId: feishuForm.appId.trim(),
        appSecret: feishuForm.appSecret.trim(),
        domain: feishuForm.domain,
        connectionMode: feishuForm.connectionMode,
        verificationToken: feishuForm.verificationToken.trim() || null,
        encryptKey: feishuForm.encryptKey.trim() || null,
      });

      if (!result.success) {
        setFormError(result.stderr || "飞书配置保存失败");
        return;
      }

      closeDialog();
      await refreshAll();
    } catch (e) {
      setFormError(`${e}`);
    } finally {
      setFormSaving(false);
    }
  };

  return (
    <TooltipProvider delayDuration={300}>
      <ScrollArea className="flex-1">
        <div className="p-5 space-y-4">
          <div className="flex items-center justify-between">
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

          {error && (
            <div className="rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-[12px] text-amber-300">
              {error}
            </div>
          )}

          {loading ? (
            <div className="flex items-center justify-center py-20 text-muted-foreground">
              <Loader2 size={18} className="animate-spin mr-2" />
              <span className="text-[13px]">加载中...</span>
            </div>
          ) : channels.length === 0 ? (
            <Card>
              <CardContent className="py-16 text-center">
                <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-violet-500/10 mx-auto mb-4">
                  <Radio size={22} className="text-violet-400" />
                </div>
                <h3 className="text-[14px] font-semibold mb-1">先接入一个飞书频道</h3>
                <p className="text-[12px] text-muted-foreground mb-4 max-w-xs mx-auto">
                  现在可以直接在控制面板里填写飞书 Bot 配置，不再需要额外打开终端。
                </p>
                <div className="flex gap-2 justify-center">
                  <Button size="sm" onClick={() => void openFeishuDialog()}>
                    <Plus size={14} /> 配置飞书
                  </Button>
                  <Button size="sm" variant="outline" asChild>
                    <a href="https://docs.openclaw.ai/channels/feishu" target="_blank" rel="noreferrer">
                      <ExternalLink size={14} /> 飞书文档
                    </a>
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
                  <Card key={key} className="group hover:border-violet-500/20 transition-colors">
                    <CardContent className="p-4">
                      <div className="flex items-center justify-between gap-3">
                        <div className="flex items-center gap-3 min-w-0">
                          <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-violet-500/10">
                            <MessageSquare size={16} className="text-violet-400" />
                          </div>
                          <div className="min-w-0">
                            <div className="flex items-center gap-2 flex-wrap">
                              <span className="text-[13px] font-semibold">{channel.name}</span>
                              <Badge variant="secondary" className={`text-[10px] px-1.5 py-0 ${info.color}`}>
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
                            <p className="text-[11px] text-muted-foreground mt-0.5">
                              账号: {channel.account}
                              {!channel.enabled && <span className="text-amber-400 ml-2">已禁用</span>}
                            </p>
                            {status?.message && (
                              <p className="text-[11px] text-muted-foreground mt-1">{status.message}</p>
                            )}
                          </div>
                        </div>
                        <div className="flex items-center gap-1 shrink-0">
                          {channel.channel === "feishu" && (
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="h-7 w-7 text-muted-foreground hover:text-sky-300 opacity-0 group-hover:opacity-100 transition-opacity"
                                  onClick={() => void openFeishuDialog(channel.account)}
                                >
                                  <Settings2 size={13} />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>编辑飞书配置</TooltipContent>
                            </Tooltip>
                          )}
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-7 w-7 text-muted-foreground hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity"
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
        </div>
      </ScrollArea>

      {dialogOpen && (
        <div className="fixed inset-0 z-[120] flex items-center justify-center bg-black/70 px-4 py-6 backdrop-blur-sm" onClick={closeDialog}>
          <Card className="w-full max-w-2xl border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40" onClick={(event) => event.stopPropagation()}>
            <CardContent className="max-h-[85vh] overflow-auto p-5 space-y-4">
              <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                <div className="space-y-1">
                  <div className="flex items-center gap-2">
                    <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-blue-500/10">
                      <MessageSquare size={15} className="text-blue-400" />
                    </div>
                    <div>
                      <h3 className="text-[13px] font-semibold">{editingAccountId ? "编辑飞书频道" : "添加飞书频道"}</h3>
                      <p className="text-[11px] text-muted-foreground">
                        直接写入 `channels.feishu` 配置；当前先支持飞书 Bot 的前端接入。
                      </p>
                    </div>
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={closeDialog} disabled={formSaving}>
                  关闭
                </Button>
              </div>

              {formLoading ? (
                <div className="flex items-center justify-center py-16 text-muted-foreground">
                  <Loader2 size={18} className="mr-2 animate-spin" />
                  <span className="text-[13px]">正在读取飞书配置...</span>
                </div>
              ) : (
                <>
                  <div className="grid gap-3 md:grid-cols-2">
                    <div>
                      <label className="mb-1.5 block text-[12px] text-muted-foreground">账号 ID</label>
                      <input
                        className={inputCls}
                        placeholder="default"
                        value={feishuForm.accountId}
                        onChange={(event) => setFeishuForm((prev) => ({ ...prev, accountId: event.target.value }))}
                      />
                    </div>
                    <div>
                      <label className="mb-1.5 block text-[12px] text-muted-foreground">显示名称</label>
                      <input
                        className={inputCls}
                        placeholder="例如 团队助手"
                        value={feishuForm.displayName}
                        onChange={(event) => setFeishuForm((prev) => ({ ...prev, displayName: event.target.value }))}
                      />
                    </div>
                    <div>
                      <label className="mb-1.5 block text-[12px] text-muted-foreground">App ID</label>
                      <input
                        className={inputCls}
                        placeholder="cli_xxx"
                        value={feishuForm.appId}
                        onChange={(event) => setFeishuForm((prev) => ({ ...prev, appId: event.target.value }))}
                      />
                    </div>
                    <div>
                      <label className="mb-1.5 block text-[12px] text-muted-foreground">App Secret</label>
                      <input
                        type="password"
                        className={inputCls}
                        placeholder="输入飞书 App Secret"
                        value={feishuForm.appSecret}
                        onChange={(event) => setFeishuForm((prev) => ({ ...prev, appSecret: event.target.value }))}
                      />
                    </div>
                    <div>
                      <DialogSelectField
                        label="域名环境"
                        menuLabel="选择飞书环境"
                        value={feishuForm.domain}
                        options={FEISHU_DOMAIN_OPTIONS}
                        open={activeDialogMenu === "domain"}
                        onOpenChange={(open) => setActiveDialogMenu(open ? "domain" : null)}
                        onValueChange={(value) => setFeishuForm((prev) => ({
                          ...prev,
                          domain: value === "lark" ? "lark" : "feishu",
                        }))}
                        menuRef={domainMenuRef}
                      />
                    </div>
                    <div>
                      <DialogSelectField
                        label="连接方式"
                        menuLabel="选择接入方式"
                        value={feishuForm.connectionMode}
                        options={FEISHU_CONNECTION_OPTIONS}
                        open={activeDialogMenu === "connection"}
                        onOpenChange={(open) => setActiveDialogMenu(open ? "connection" : null)}
                        onValueChange={(value) => setFeishuForm((prev) => ({
                          ...prev,
                          connectionMode: value === "webhook" ? "webhook" : "websocket",
                        }))}
                        menuRef={connectionMenuRef}
                      />
                    </div>
                  </div>

                  {feishuForm.connectionMode === "webhook" && (
                    <div className="grid gap-3 md:grid-cols-2">
                      <div>
                        <label className="mb-1.5 block text-[12px] text-muted-foreground">Verification Token</label>
                        <input
                          className={inputCls}
                          placeholder="Webhook 模式必填"
                          value={feishuForm.verificationToken}
                          onChange={(event) => setFeishuForm((prev) => ({ ...prev, verificationToken: event.target.value }))}
                        />
                      </div>
                      <div>
                        <label className="mb-1.5 block text-[12px] text-muted-foreground">Encrypt Key</label>
                        <input
                          type="password"
                          className={inputCls}
                          placeholder="Webhook 模式必填"
                          value={feishuForm.encryptKey}
                          onChange={(event) => setFeishuForm((prev) => ({ ...prev, encryptKey: event.target.value }))}
                        />
                      </div>
                    </div>
                  )}

                  <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[11px] text-muted-foreground">
                    <p>保存后会直接写入 `~/.openclaw/openclaw.json` 的 `channels.feishu` 配置。</p>
                    <p className="mt-1">
                      如果你还没安装飞书插件，仍需额外执行 `openclaw plugins install @openclaw/feishu`。
                    </p>
                  </div>

                  {formError && (
                    <div className="rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                      {formError}
                    </div>
                  )}

                  <div className="flex flex-wrap gap-2">
                    <Button size="sm" onClick={() => void handleSaveFeishu()} disabled={formSaving || !canSaveFeishu}>
                      {formSaving ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                      {formSaving ? "保存中..." : editingAccountId ? "保存配置" : "添加飞书"}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      asChild
                    >
                      <a href="https://docs.openclaw.ai/channels/feishu" target="_blank" rel="noreferrer">
                        <ExternalLink size={14} />
                        查看飞书文档
                      </a>
                    </Button>
                  </div>
                </>
              )}
            </CardContent>
          </Card>
        </div>
      )}
    </TooltipProvider>
  );
}
