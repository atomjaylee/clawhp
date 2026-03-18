import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
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

type FeishuTerminalAction = "install" | "update" | "doctor";

const FEISHU_GUIDE_URL = "https://bytedance.larkoffice.com/docx/MFK7dDFLFoVlOGxWCv5cTXKmnMh";
const FEISHU_ARTICLE_URL = "https://www.feishu.cn/content/article/7613711414611463386";

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

const FEISHU_ACTIONS: Array<{
  intent: "existing" | "new";
  title: string;
  description: string;
  hint: string;
}> = [
  {
    intent: "existing",
    title: "关联已有机器人",
    description: "适合你已经在飞书开放平台或之前的插件流程里创建过机器人，只想重新绑定到当前 OpenClaw。",
    hint: "打开终端后，在安装流程里选择“关联已有机器人”，再用飞书扫码完成绑定。",
  },
  {
    intent: "new",
    title: "新建机器人",
    description: "适合从零开始接入，扫码后可在飞书里一键创建新的 OpenClaw 机器人。",
    hint: "打开终端后，在安装流程里选择“新建机器人”，扫码后继续完成创建。",
  },
];

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
  const [actionLoading, setActionLoading] = useState<FeishuTerminalAction | null>(null);
  const [actionMessage, setActionMessage] = useState<{ tone: "success" | "error"; text: string } | null>(null);

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

  const refreshAll = useCallback(async () => {
    await Promise.all([fetchChannels(), fetchStatus()]);
  }, [fetchChannels, fetchStatus]);

  const getStatus = useCallback((channel: string, account: string) => (
    statuses.find((entry) => entry.channel === channel && (entry.account === account || entry.account === "default"))
  ), [statuses]);

  const openFeishuDialog = useCallback((accountId?: string) => {
    setEditingAccountId(accountId ?? null);
    setActionMessage(null);
    setDialogOpen(true);
  }, []);

  const closeDialog = useCallback(() => {
    if (actionLoading) {
      return;
    }
    setDialogOpen(false);
    setEditingAccountId(null);
    setActionMessage(null);
  }, [actionLoading]);

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

  const handleFeishuAction = useCallback(async (action: FeishuTerminalAction) => {
    setActionLoading(action);
    setActionMessage(null);
    try {
      const result: CommandResult = await invoke("open_feishu_plugin_terminal", { action });
      if (!result.success) {
        setActionMessage({
          tone: "error",
          text: result.stderr || "飞书插件命令启动失败",
        });
        return;
      }
      const suffix = action === "install"
        ? "完成扫码和终端向导后，回到这里点“刷新”即可看到新的频道。"
        : "命令已经在外部终端启动，可以在终端里继续完成操作。";
      setActionMessage({
        tone: "success",
        text: `${result.stdout}。${suffix}`,
      });
    } catch (e) {
      setActionMessage({
        tone: "error",
        text: `${e}`,
      });
    } finally {
      setActionLoading(null);
    }
  }, []);

  return (
    <TooltipProvider delayDuration={300}>
      <ScrollArea className="flex-1">
        <div className="space-y-4 p-5">
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
              <Button size="sm" variant="outline" onClick={() => openFeishuDialog()}>
                <Plus size={14} /> 接入飞书
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
                  频道页已经切到飞书官方插件流程，不再手动填写 App ID / App Secret，支持扫码绑定已有机器人或新建机器人。
                </p>
                <div className="flex justify-center gap-2">
                  <Button size="sm" onClick={() => openFeishuDialog()}>
                    <Plus size={14} /> 开始扫码绑定
                  </Button>
                  <Button size="sm" variant="outline" asChild>
                    <a href={FEISHU_GUIDE_URL} target="_blank" rel="noreferrer">
                      <ExternalLink size={14} /> 查看对接文档
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
                                  onClick={() => openFeishuDialog(channel.account)}
                                >
                                  <Settings2 size={13} />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>扫码新增或管理飞书插件</TooltipContent>
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
        </div>
      </ScrollArea>

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
                      <h3 className="text-[13px] font-semibold">{editingAccountId ? "管理飞书接入" : "接入飞书官方插件"}</h3>
                      <p className="text-[11px] text-muted-foreground">
                        通过官方插件安装命令完成扫码绑定；终端里可选择关联已有机器人，也可新建机器人。
                      </p>
                    </div>
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={closeDialog} disabled={Boolean(actionLoading)}>
                  关闭
                </Button>
              </div>

              <div className="rounded-2xl border border-sky-500/15 bg-sky-500/8 p-4">
                <div className="flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
                  <div className="space-y-1">
                    <p className="text-[11px] uppercase tracking-[0.18em] text-sky-200/70">扫码绑定流程</p>
                    <h4 className="text-[16px] font-semibold text-sky-50">打开终端后执行官方安装向导</h4>
                    <p className="max-w-2xl text-[12px] text-sky-100/75">
                      安装命令会拉起飞书官方插件流程。你只需要在终端里选择绑定方式，然后用飞书客户端扫码，后续配置由插件自动完成。
                    </p>
                  </div>
                  <Button size="sm" onClick={() => void handleFeishuAction("install")} disabled={actionLoading === "install"}>
                    {actionLoading === "install" ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                    开始扫码绑定
                  </Button>
                </div>
                <div className="mt-3 rounded-xl border border-white/[0.06] bg-black/20 px-3 py-2 font-mono text-[11px] text-sky-100/80">
                  npx -y @larksuite/openclaw-lark-tools install
                </div>
              </div>

              <div className="grid gap-3 md:grid-cols-2">
                {FEISHU_ACTIONS.map((item) => (
                  <Card key={item.intent} className="border-white/[0.08] bg-white/[0.02]">
                    <CardContent className="space-y-3 p-4">
                      <div className="space-y-1">
                        <h4 className="text-[13px] font-semibold">{item.title}</h4>
                        <p className="text-[12px] leading-5 text-muted-foreground">{item.description}</p>
                      </div>
                      <div className="rounded-xl border border-white/[0.06] bg-white/[0.02] p-3 text-[11px] leading-5 text-muted-foreground">
                        {item.hint}
                      </div>
                      <Button size="sm" variant="outline" onClick={() => void handleFeishuAction("install")} disabled={actionLoading === "install"}>
                        {actionLoading === "install" ? <Loader2 className="animate-spin" /> : <Plus size={14} />}
                        打开终端扫码
                      </Button>
                    </CardContent>
                  </Card>
                ))}
              </div>

              <div className="grid gap-3 lg:grid-cols-[1.4fr_1fr]">
                <Card className="border-white/[0.08] bg-white/[0.02]">
                  <CardContent className="space-y-3 p-4">
                    <h4 className="text-[13px] font-semibold">完成绑定后怎么验证</h4>
                    <div className="space-y-2 text-[12px] leading-5 text-muted-foreground">
                      <p>1. 在飞书里打开新绑定的机器人，先发送任意消息确认它已经在线。</p>
                      <p>2. 如果希望 OpenClaw 以你的身份读写消息、文档、日程等，可以在飞书里发送 <code>/feishu auth</code> 完成授权。</p>
                      <p>3. 发送 <code>/feishu start</code> 检查插件版本和运行状态；回来点页面右上角“刷新”同步频道列表。</p>
                    </div>
                  </CardContent>
                </Card>

                <Card className="border-white/[0.08] bg-white/[0.02]">
                  <CardContent className="space-y-3 p-4">
                    <h4 className="text-[13px] font-semibold">常用维护命令</h4>
                    <div className="flex flex-col gap-2">
                      <Button size="sm" variant="outline" onClick={() => void handleFeishuAction("update")} disabled={actionLoading === "update"}>
                        {actionLoading === "update" ? <Loader2 className="animate-spin" /> : <RefreshCw size={14} />}
                        更新插件
                      </Button>
                      <Button size="sm" variant="outline" onClick={() => void handleFeishuAction("doctor")} disabled={actionLoading === "doctor"}>
                        {actionLoading === "doctor" ? <Loader2 className="animate-spin" /> : <Settings2 size={14} />}
                        诊断问题
                      </Button>
                      <Button size="sm" variant="outline" asChild>
                        <a href={FEISHU_GUIDE_URL} target="_blank" rel="noreferrer">
                          <ExternalLink size={14} />
                          查看对接文档
                        </a>
                      </Button>
                      <Button size="sm" variant="outline" asChild>
                        <a href={FEISHU_ARTICLE_URL} target="_blank" rel="noreferrer">
                          <ExternalLink size={14} />
                          官方安装说明
                        </a>
                      </Button>
                    </div>
                  </CardContent>
                </Card>
              </div>

              {actionMessage && (
                <div
                  className={`rounded-lg px-3 py-2.5 text-[12px] ${
                    actionMessage.tone === "success"
                      ? "border border-emerald-500/20 bg-emerald-500/10 text-emerald-200"
                      : "border border-red-500/20 bg-red-500/8 text-red-300"
                  }`}
                >
                  {actionMessage.text}
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      )}
    </TooltipProvider>
  );
}
