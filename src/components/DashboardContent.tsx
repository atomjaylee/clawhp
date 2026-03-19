import { useState, useEffect, useCallback, type ComponentProps, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowRight,
  BookOpen,
  CircleAlert,
  CircleCheck,
  CircleX,
  ExternalLink,
  FileText,
  Globe,
  Loader2,
  Play,
  RefreshCw,
  Server,
  Square,
  Terminal,
  Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import ModuleTabs, { type ModuleTabItem } from "@/components/ui/module-tabs";
import PageShell from "@/components/PageShell";
import type { SystemInfo, CommandResult, DashboardTab } from "@/types";

interface Props {
  systemInfo: SystemInfo;
  onNavigate?: (tab: DashboardTab) => void;
}

type GatewayStatus = "unknown" | "checking" | "running" | "stopped" | "starting" | "stopping" | "recovering";
type Tone = "neutral" | "good" | "warn" | "danger";
type DashboardModuleTab = "overview" | "actions" | "system";

const PORT_POLL_INTERVAL = 5000;
const SNAPSHOT_POLL_INTERVAL = 30000;

interface GatewayLogEvent {
  level: string;
  message: string;
}

interface GatewaySnapshot {
  service?: {
    runtime?: {
      status?: string;
      state?: string;
    };
  };
  config?: {
    cli?: {
      path?: string;
    };
  };
  gateway?: {
    port?: number;
    probeNote?: string;
  };
  rpc?: {
    ok?: boolean;
    error?: string;
  };
}

interface SecurityAuditSnapshot {
  summary?: {
    critical?: number;
    warn?: number;
    info?: number;
  };
  findings?: SecurityFinding[];
}

interface SecurityFinding {
  severity?: "critical" | "warn" | "info" | string;
  title?: string;
  detail?: string;
  remediation?: string;
}

interface RuntimeSnapshot {
  runtimeVersion?: string;
  channelSummary?: unknown[];
  sessions?: {
    defaults?: {
      model?: string;
    };
    recent?: Array<{
      model?: string;
    }>;
  };
  gatewayService?: {
    runtimeShort?: string;
  };
  securityAudit?: SecurityAuditSnapshot;
}

interface SetupStep {
  title: string;
  description: string;
  done: boolean;
  actionLabel: string;
  onAction: () => void | Promise<void>;
}

interface AttentionItem {
  title: string;
  body: string;
  actionLabel: string;
  onAction: () => void | Promise<void>;
}

export default function DashboardContent({ systemInfo, onNavigate }: Props) {
  const osLabel = { macos: "macOS", linux: "Linux", windows: "Windows" }[systemInfo.os] ?? systemInfo.os;
  const gatewayPort = systemInfo.gateway_port ?? 18789;
  const configPath = systemInfo.openclaw_config_path ?? "~/.openclaw/openclaw.json";
  const configFileLabel = getPathTail(configPath);

  const [gwStatus, setGwStatus] = useState<GatewayStatus>("unknown");
  const [gwMessage, setGwMessage] = useState("");
  const [gwLogs, setGwLogs] = useState<string[]>([]);
  const [showLogs, setShowLogs] = useState(false);
  const [statusLoading, setStatusLoading] = useState(false);
  const [statusError, setStatusError] = useState("");
  const [gatewaySnapshot, setGatewaySnapshot] = useState<GatewaySnapshot | null>(null);
  const [runtimeSnapshot, setRuntimeSnapshot] = useState<RuntimeSnapshot | null>(null);
  const [securityAudit, setSecurityAudit] = useState<SecurityAuditSnapshot | null>(null);
  const [configuredPrimaryModel, setConfiguredPrimaryModel] = useState<string | null>(null);
  const [openingDashboard, setOpeningDashboard] = useState(false);
  const [configuredChannelCount, setConfiguredChannelCount] = useState(0);
  const [moduleTab, setModuleTab] = useState<DashboardModuleTab>("overview");

  const checkGateway = useCallback(async () => {
    try {
      const r: CommandResult = await invoke("check_gateway_port", { port: gatewayPort });
      setGwStatus(r.success ? "running" : "stopped");
    } catch {
      setGwStatus("stopped");
    }
  }, [gatewayPort]);

  const refreshConfiguredChannels = useCallback(async () => {
    try {
      const result: CommandResult = await invoke("list_channels_snapshot");
      setConfiguredChannelCount(countConfiguredChannelsFromSnapshot(result));
    } catch {
      setConfiguredChannelCount(0);
    }
  }, []);

  const refreshSnapshots = useCallback(async () => {
    setStatusLoading(true);
    const errors: string[] = [];

    try {
      const [gatewayResult, runtimeResult, auditResult] = await Promise.allSettled([
        invoke<CommandResult>("get_gateway_status_snapshot"),
        invoke<CommandResult>("get_runtime_status_snapshot"),
        invoke<CommandResult>("get_security_audit_snapshot"),
      ]);

      let nextGateway: GatewaySnapshot | null = null;
      let nextRuntime: RuntimeSnapshot | null = null;
      let nextAudit: SecurityAuditSnapshot | null = null;

      if (gatewayResult.status === "fulfilled") {
        nextGateway = parseJsonResult<GatewaySnapshot>(gatewayResult.value);
        if (!nextGateway) {
          errors.push(gatewayResult.value.stderr || "无法读取网关状态");
        }
      } else {
        errors.push("网关状态获取失败");
      }

      if (runtimeResult.status === "fulfilled") {
        nextRuntime = parseJsonResult<RuntimeSnapshot>(runtimeResult.value);
        if (!nextRuntime) {
          errors.push(runtimeResult.value.stderr || "无法读取运行状态");
        }
      } else {
        errors.push("运行状态获取失败");
      }

      if (auditResult.status === "fulfilled") {
        nextAudit = parseJsonResult<SecurityAuditSnapshot>(auditResult.value);
        if (!nextAudit && !nextRuntime?.securityAudit) {
          errors.push(auditResult.value.stderr || "无法读取安全提醒");
        }
      } else {
        errors.push("安全提醒获取失败");
      }

      if (nextGateway) {
        setGatewaySnapshot(nextGateway);
      }
      if (nextRuntime) {
        setRuntimeSnapshot(nextRuntime);
      }
      if (nextAudit || nextRuntime?.securityAudit) {
        setSecurityAudit(nextAudit ?? nextRuntime?.securityAudit ?? null);
      }

      if (nextGateway?.rpc?.ok === false) {
        setGwMessage(firstMeaningfulLine(nextGateway.rpc.error) ?? "网关已启动，但控制面板暂时连不上");
      } else if (nextGateway?.gateway?.probeNote) {
        setGwMessage((current) => current || nextGateway?.gateway?.probeNote || "");
      }

      setStatusError(compactMessages(errors).join(" "));
    } catch (error) {
      setStatusError(`${error}`);
    } finally {
      setStatusLoading(false);
    }
  }, []);

  const refreshConfiguredPrimaryModel = useCallback(async () => {
    try {
      const nextPrimary = await invoke<string>("get_primary_model");
      setConfiguredPrimaryModel(nextPrimary || null);
    } catch {
      setConfiguredPrimaryModel(null);
    }
  }, []);

  useEffect(() => {
    setGwStatus("checking");
    checkGateway();
    refreshSnapshots();
    void refreshConfiguredChannels();
    void refreshConfiguredPrimaryModel();

    const portTimer = setInterval(checkGateway, PORT_POLL_INTERVAL);
    const snapshotTimer = setInterval(refreshSnapshots, SNAPSHOT_POLL_INTERVAL);
    const channelTimer = setInterval(() => {
      void refreshConfiguredChannels();
    }, SNAPSHOT_POLL_INTERVAL);
    const primaryTimer = setInterval(() => {
      void refreshConfiguredPrimaryModel();
    }, SNAPSHOT_POLL_INTERVAL);

    return () => {
      clearInterval(portTimer);
      clearInterval(snapshotTimer);
      clearInterval(channelTimer);
      clearInterval(primaryTimer);
    };
  }, [checkGateway, refreshSnapshots, refreshConfiguredChannels, refreshConfiguredPrimaryModel]);

  useEffect(() => {
    const unlisten = listen<GatewayLogEvent>("gateway-log", (event) => {
      const { level, message } = event.payload;
      setGwLogs((prev) => [...prev.slice(-49), `[${level}] ${message}`]);

      if (level === "info" && message.includes("就绪")) {
        setGwStatus("running");
        setGwMessage(message);
        void refreshSnapshots();
        void refreshConfiguredChannels();
      } else if (level === "warn") {
        setGwStatus("recovering");
        setGwMessage(message);
      } else if (level === "error") {
        setGwStatus("stopped");
        setGwMessage(message);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshSnapshots]);

  const handleStartGateway = async () => {
    setGwStatus("starting");
    setGwMessage("");
    setGwLogs([]);
    try {
      const r: CommandResult = await invoke("start_gateway_with_recovery", { port: gatewayPort });
      if (r.success) {
        setGwStatus("running");
        setGwMessage(firstMeaningfulLine(r.stdout) ?? "网关已启动");
      } else {
        setGwStatus("stopped");
        setGwMessage(r.stderr || "启动失败");
      }
    } catch (e) {
      setGwStatus("stopped");
      setGwMessage(`${e}`);
    } finally {
      await checkGateway();
      await refreshSnapshots();
    }
  };

  const handleStopGateway = async () => {
    setGwStatus("stopping");
    setGwMessage("");
    try {
      const r: CommandResult = await invoke("run_openclaw_command", { args: ["gateway", "stop"] });
      setGwMessage(r.success ? "网关已停止" : (r.stderr || "停止失败"));
    } catch (e) {
      setGwMessage(`${e}`);
    } finally {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      await checkGateway();
      await refreshSnapshots();
    }
  };

  const handleViewLogs = async () => {
    if (showLogs) {
      setShowLogs(false);
      return;
    }
    try {
      const r: CommandResult = await invoke("get_gateway_logs");
      if (r.success) {
        setGwLogs(r.stdout.split("\n"));
      }
    } catch {
      // ignore
    }
    setShowLogs(true);
  };

  const handleOpenDashboard = async () => {
    if (openingDashboard) {
      return;
    }

    setOpeningDashboard(true);
    try {
      try {
        const tokenResult: CommandResult = await invoke("get_gateway_token");
        const token = tokenResult.success ? tokenResult.stdout.trim() : "";
        const controlUiUrl = token
          ? `http://127.0.0.1:${gatewayPort}/?token=${token}`
          : `http://127.0.0.1:${gatewayPort}/`;
        const popup = window.open(controlUiUrl, "_blank", "noopener,noreferrer");

        if (popup) {
          setGwMessage("已在浏览器中打开本地 Control UI");
          return;
        }

        const r: CommandResult = await invoke("open_dashboard");
        if (r.success) {
          setGwMessage(firstMeaningfulLine(r.stdout) ?? "Control UI 已在浏览器中打开");
          return;
        }
      } catch {
        // fall through to the direct URL fallback below
      }

      window.open(`http://127.0.0.1:${gatewayPort}/`, "_blank", "noopener,noreferrer");
      setGwMessage("已尝试直接打开本地 Control UI");
    } finally {
      setOpeningDashboard(false);
    }
  };

  const gatewayData = gatewaySnapshot?.gateway;
  const gatewayServiceLabel = runtimeSnapshot?.gatewayService?.runtimeShort
    ?? gatewaySnapshot?.service?.runtime?.status
    ?? (gwStatus === "running" ? "running" : "stopped");
  const channelCount = configuredChannelCount;
  const defaultModel = configuredPrimaryModel
    ?? runtimeSnapshot?.sessions?.defaults?.model
    ?? runtimeSnapshot?.sessions?.recent?.[0]?.model
    ?? "未检测";
  const securitySummary = securityAudit?.summary ?? runtimeSnapshot?.securityAudit?.summary;
  const securityFindings = securityAudit?.findings ?? runtimeSnapshot?.securityAudit?.findings ?? [];
  const topFinding = securityFindings.find((finding) => finding.severity !== "info") ?? securityFindings[0];
  const gatewayProbeError = firstMeaningfulLine(gatewaySnapshot?.rpc?.error);
  const gwRunning = gwStatus === "running";
  const gwBusy = gwStatus === "starting" || gwStatus === "stopping" || gwStatus === "checking" || gwStatus === "recovering";
  const riskCount = (securitySummary?.critical ?? 0) + (securitySummary?.warn ?? 0);
  const modelConfigured = defaultModel !== "未检测";
  const controlUiReady = gwRunning && gatewaySnapshot?.rpc?.ok !== false;

  const setupSteps: SetupStep[] = [
    {
      title: "先让网关跑起来",
      description: "网关启动后，Control UI 和频道接入才会正常工作。",
      done: gwRunning,
      actionLabel: gwRunning ? "打开面板" : "启动网关",
      onAction: gwRunning ? handleOpenDashboard : () => void handleStartGateway(),
    },
    {
      title: "配置一个主模型",
      description: "先有主模型，后面的聊天和 Agent 才能真正可用。",
      done: modelConfigured,
      actionLabel: modelConfigured ? "管理模型" : "去配模型",
      onAction: () => onNavigate?.("models"),
    },
    {
      title: "接入第一个频道",
      description: "如果你想从聊天入口使用 OpenClaw，这一步最关键。",
      done: channelCount > 0,
      actionLabel: channelCount > 0 ? "管理频道" : "去配频道",
      onAction: () => onNavigate?.("channels"),
    },
  ];

  const attentionItems: AttentionItem[] = [];

  if (!gwRunning) {
    attentionItems.push({
      title: "网关还没启动",
      body: "先点“启动网关”，启动成功后再打开 Control UI。",
      actionLabel: "启动网关",
      onAction: () => void handleStartGateway(),
    });
  }

  if (!modelConfigured) {
    attentionItems.push({
      title: "还没有可用主模型",
      body: "先到模型管理里添加 Provider，并设一个主模型。",
      actionLabel: "去模型管理",
      onAction: () => onNavigate?.("models"),
    });
  }

  if (channelCount === 0) {
    attentionItems.push({
      title: "还没有接入频道",
      body: "如果你希望从聊天工具里使用 OpenClaw，下一步就是接频道。",
      actionLabel: "去配频道",
      onAction: () => onNavigate?.("channels"),
    });
  }

  if (!systemInfo.openclaw_doctor_ok) {
    attentionItems.push({
      title: "健康检查还没通过",
      body: "建议先去设置页运行 onboard 或检查安装状态，避免后续功能一会儿能用一会儿不能用。",
      actionLabel: "打开设置",
      onAction: () => onNavigate?.("settings"),
    });
  }

  if (riskCount > 0) {
    attentionItems.push({
      title: topFinding?.title ?? "还有需要处理的提醒",
      body: firstMeaningfulLine(topFinding?.remediation ?? topFinding?.detail) ?? `当前还有 ${riskCount} 项提醒，建议先处理。`,
      actionLabel: "查看设置",
      onAction: () => onNavigate?.("settings"),
    });
  }
  const moduleTabs: ModuleTabItem<DashboardModuleTab>[] = [
    { id: "overview", label: "上手指南", icon: BookOpen, badge: `${setupSteps.filter((step) => step.done).length}/${setupSteps.length}` },
    { id: "actions", label: "常用入口", icon: ArrowRight, badge: attentionItems.length || "OK" },
    { id: "system", label: "环境信息", icon: Wrench, badge: riskCount || "稳" },
  ];

  return (
    <PageShell
      bodyClassName="space-y-5"
      header={(
        <div className="flex items-center justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold">仪表盘</h2>
            <p className="text-[11px] text-muted-foreground">
              聚合网关、模型和安全状态，优先显示你现在最需要处理的事情。
            </p>
          </div>
          <Button
            size="sm"
            variant="outline"
            onClick={async () => {
              setGwStatus("checking");
              await Promise.all([checkGateway(), refreshSnapshots(), refreshConfiguredChannels(), refreshConfiguredPrimaryModel()]);
            }}
            disabled={gwBusy || statusLoading}
          >
            {statusLoading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {statusLoading ? "同步中..." : "刷新状态"}
          </Button>
        </div>
      )}
    >
        <Card className={gwRunning ? "border-emerald-500/20" : "border-amber-500/20"}>
          <CardContent className="p-5">
            <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
              <div className="flex items-start gap-3">
                <div className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl ${
                  gwRunning ? "bg-emerald-500/10" : "bg-amber-500/10"
                }`}>
                  <Server size={20} className={gwRunning ? "text-emerald-400" : "text-amber-400"} />
                </div>
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="text-[14px] font-semibold">
                      {controlUiReady && modelConfigured ? "OpenClaw 基本就绪" : "还差几步才能真正开始用"}
                    </h3>
                    <StatusBadge tone={controlUiReady && modelConfigured ? "good" : gwBusy ? "neutral" : "warn"}>
                      {gwStatus === "checking" ? "检测中"
                        : gwStatus === "starting" ? "启动中"
                        : gwStatus === "stopping" ? "停止中"
                        : gwStatus === "recovering" ? "自动修复中"
                        : gwRunning ? "网关运行中"
                        : "网关未运行"}
                    </StatusBadge>
                    {statusLoading && (
                      <Badge variant="outline" className="border-white/[0.08] bg-white/[0.03] text-muted-foreground">
                        <Loader2 size={12} className="mr-1 animate-spin" />
                        同步状态
                      </Badge>
                    )}
                  </div>
                  <p className="mt-1 text-[12px] text-muted-foreground">
                    先启动网关，再配置主模型和频道。首页现在只保留真正会影响你能不能开始使用的内容。
                  </p>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <Button size="sm" className="gap-1.5" onClick={handleOpenDashboard}>
                  {openingDashboard ? <Loader2 size={13} className="animate-spin" /> : <Globe size={13} />}
                  {openingDashboard ? "打开中..." : "打开 Control UI"}
                </Button>
                {(gwStatus === "running" || gwStatus === "stopping") ? (
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={handleStopGateway}
                    disabled={gwBusy}
                    className="border-red-500/20 text-red-400 hover:bg-red-500/10 hover:text-red-400"
                  >
                    {gwStatus === "stopping" ? <Loader2 size={13} className="animate-spin" /> : <Square size={13} />}
                    停止
                  </Button>
                ) : (
                  <Button size="sm" variant="outline" onClick={handleStartGateway} disabled={gwBusy}>
                    {gwStatus === "starting" ? <Loader2 size={13} className="animate-spin" /> : <Play size={13} />}
                    启动网关
                  </Button>
                )}
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-8 w-8 text-muted-foreground"
                  onClick={async () => {
                    setGwStatus("checking");
                    await Promise.all([checkGateway(), refreshSnapshots(), refreshConfiguredChannels(), refreshConfiguredPrimaryModel()]);
                  }}
                  disabled={gwBusy || statusLoading}
                  title="刷新状态"
                >
                  <RefreshCw size={13} />
                </Button>
                <Button
                  size="icon"
                  variant="ghost"
                  className="h-8 w-8 text-muted-foreground"
                  onClick={handleViewLogs}
                  title="查看网关日志"
                >
                  <FileText size={13} />
                </Button>
              </div>
            </div>

            <div className="mt-4 grid grid-cols-2 gap-2 xl:grid-cols-4">
              <SnapshotChip label="网关" value={gatewayServiceLabel} tone={gwRunning ? "good" : "warn"} />
              <SnapshotChip label="Control UI" value={controlUiReady ? "可打开" : "未就绪"} tone={controlUiReady ? "good" : "warn"} />
              <SnapshotChip label="端口" value={`${gatewayData?.port ?? gatewayPort}`} />
              <SnapshotChip label="配置文件" value={getPathTail(gatewaySnapshot?.config?.cli?.path ?? configPath)} />
            </div>

            {gwMessage && (
              <pre className="mt-3 rounded-lg bg-white/[0.03] border border-white/[0.06] p-2.5 text-[11px] font-mono text-muted-foreground whitespace-pre-wrap max-h-24 overflow-auto">
                {gwMessage}
              </pre>
            )}

            {gatewayProbeError && (
              <InlineNote
                className="mt-3"
                tone={gwRunning ? "warn" : "neutral"}
                title={gwRunning ? "网关进程在运行，但控制面板探针返回异常" : "当前无法连接到本地网关"}
                body={gatewayProbeError}
              />
            )}

            {showLogs && gwLogs.length > 0 && (
              <div className="mt-3 rounded-lg bg-white/[0.03] border border-white/[0.06] overflow-hidden">
                <div className="flex items-center justify-between px-3 py-1.5 border-b border-white/[0.06]">
                  <div className="flex items-center gap-1.5">
                    <Terminal size={11} className="text-muted-foreground" />
                    <span className="text-[11px] text-muted-foreground font-medium">网关日志</span>
                  </div>
                  <button
                    onClick={() => setShowLogs(false)}
                    className="text-[10px] text-muted-foreground hover:text-foreground"
                  >
                    关闭
                  </button>
                </div>
                <ScrollArea className="max-h-40">
                  <pre className="p-2.5 text-[10px] font-mono text-muted-foreground whitespace-pre-wrap">
                    {gwLogs.join("\n")}
                  </pre>
                </ScrollArea>
              </div>
            )}
          </CardContent>
        </Card>

        <ModuleTabs items={moduleTabs} value={moduleTab} onValueChange={setModuleTab} />

        {moduleTab === "overview" && (
          <>
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-[1.2fr_0.8fr]">
          <Card>
            <CardContent className="p-5">
              <div className="flex items-center justify-between mb-4">
                <div>
                  <h3 className="text-[13px] font-semibold text-foreground/90">上手 3 步</h3>
                  <p className="text-[11px] text-muted-foreground mt-1">
                    新手先把下面这三步做完，比看一堆运行指标更有用。
                  </p>
                </div>
                <StatusBadge tone={setupSteps.every((step) => step.done) ? "good" : "warn"}>
                  {setupSteps.filter((step) => step.done).length}/{setupSteps.length} 已完成
                </StatusBadge>
              </div>

              <div className="space-y-3">
                {setupSteps.map((step) => (
                  <BeginnerStepCard
                    key={step.title}
                    title={step.title}
                    description={step.description}
                    done={step.done}
                    actionLabel={step.actionLabel}
                    onAction={step.onAction}
                  />
                ))}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardContent className="p-5">
              <div className="flex items-center justify-between mb-4">
                <div>
                  <h3 className="text-[13px] font-semibold text-foreground/90">当前状态</h3>
                  <p className="text-[11px] text-muted-foreground mt-1">
                    这里只保留会直接影响你能不能开始使用的几个状态。
                  </p>
                </div>
                <StatusBadge tone={controlUiReady && modelConfigured && channelCount > 0 && systemInfo.openclaw_doctor_ok ? "good" : "warn"}>
                  {controlUiReady && modelConfigured && channelCount > 0 && systemInfo.openclaw_doctor_ok ? "基本就绪" : "还需配置"}
                </StatusBadge>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <SnapshotChip label="主模型" value={modelConfigured ? defaultModel : "未配置"} tone={modelConfigured ? "good" : "warn"} />
                <SnapshotChip label="频道" value={channelCount > 0 ? `${channelCount} 个已接入` : "未接入"} tone={channelCount > 0 ? "good" : "warn"} />
                <SnapshotChip label="Doctor" value={systemInfo.openclaw_doctor_ok ? "通过" : "未通过"} tone={systemInfo.openclaw_doctor_ok ? "good" : "warn"} />
                <SnapshotChip label="系统" value={systemInfo.openclaw_fully_installed ? "安装完整" : "待修复"} tone={systemInfo.openclaw_fully_installed ? "good" : "warn"} />
              </div>

              {statusError ? (
                <InlineNote
                  className="mt-4"
                  tone="warn"
                  title="状态还没完全读出来"
                  body={statusError}
                />
              ) : (
                <InlineNote
                  className="mt-4"
                  tone="neutral"
                  title={controlUiReady ? "现在可以直接开始用了" : "先把上面的步骤补齐"}
                  body={controlUiReady
                    ? "你现在可以打开 Control UI，继续在模型、频道和设置页里完成更细的配置。"
                    : "如果你不知道下一步点哪里，就从“启动网关”或“去配模型”开始。"}
                />
              )}
            </CardContent>
          </Card>
        </div>
          </>
        )}

        {moduleTab === "actions" && (
          <>
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-[0.9fr_1.1fr]">
          <Card className={attentionItems.length > 0 ? "border-amber-500/20" : "border-emerald-500/20"}>
            <CardContent className="p-5">
              <div className="flex items-center justify-between mb-4">
                <div className="flex items-center gap-2">
                  <CircleAlert size={15} className={attentionItems.length > 0 ? "text-amber-400" : "text-emerald-400"} />
                  <h3 className="text-[13px] font-semibold text-foreground/90">建议先处理</h3>
                </div>
                <StatusBadge tone={attentionItems.length > 0 ? "warn" : "good"}>
                  {attentionItems.length > 0 ? `${attentionItems.length} 项` : "无阻塞项"}
                </StatusBadge>
              </div>

              {attentionItems.length > 0 ? (
                <div className="space-y-3">
                  {attentionItems.map((item) => (
                    <div key={item.title} className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                      <p className="text-[12px] font-medium text-foreground/90">{item.title}</p>
                      <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">{item.body}</p>
                      <AsyncActionButton size="sm" variant="outline" className="mt-3" onAction={item.onAction}>
                        {item.actionLabel}
                        <ArrowRight size={13} />
                      </AsyncActionButton>
                    </div>
                  ))}
                </div>
              ) : (
                <InlineNote
                  tone="good"
                  title="首页没有明显阻塞项"
                  body="如果你只是想开始体验，直接打开 Control UI 就够了；更细的配置再去模型、频道和设置页慢慢补。"
                />
              )}
            </CardContent>
          </Card>

          <Card>
            <CardContent className="p-5">
              <div className="flex items-center justify-between mb-4">
                <div>
                  <h3 className="text-[13px] font-semibold text-foreground/90">常用入口</h3>
                  <p className="text-[11px] text-muted-foreground mt-1">
                    首页只保留最常用的入口，更多细节放到各自页面里。
                  </p>
                </div>
                <StatusBadge tone="neutral">
                  新手常用
                </StatusBadge>
              </div>

              <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                <ActionLinkCard
                  icon={Globe}
                  title="打开 Control UI"
                  desc="真正开始使用 OpenClaw 的主入口"
                  onClick={handleOpenDashboard}
                />
                <ActionLinkCard
                  icon={Wrench}
                  title="设置与恢复"
                  desc="运行 onboard、检查安装和处理异常"
                  onClick={onNavigate ? () => onNavigate("settings") : undefined}
                />
                <ActionLinkCard
                  icon={BookOpen}
                  title="模型管理"
                  desc={modelConfigured ? `当前主模型：${defaultModel}` : "先添加 Provider 并设一个主模型"}
                  onClick={onNavigate ? () => onNavigate("models") : undefined}
                />
                <ActionLinkCard
                  icon={FileText}
                  title={channelCount > 0 ? "频道管理" : "配置第一个频道"}
                  desc={channelCount > 0 ? `当前已接入 ${channelCount} 个频道` : "要从聊天入口用 OpenClaw，先接频道"}
                  onClick={onNavigate ? () => onNavigate("channels") : undefined}
                />
                <ActionLinkCard
                  icon={ExternalLink}
                  title="新版入门文档"
                  desc="跟官方步骤核对安装、状态和下一步"
                  href="https://docs.openclaw.ai/start/getting-started"
                />
              </div>
            </CardContent>
          </Card>
        </div>
          </>
        )}

        {moduleTab === "system" && (
          <>
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          <Card>
            <CardContent className="p-5">
              <h3 className="text-[13px] font-semibold text-foreground/90 mb-4">基础信息</h3>

              <div className="space-y-3.5">
                <DataRow label="OpenClaw 版本" value={systemInfo.openclaw_version ?? "未检测"} ok={systemInfo.openclaw_cli_ok} />
                <DataRow label="安装状态" value={systemInfo.openclaw_fully_installed ? "完整" : "不完整"} ok={systemInfo.openclaw_fully_installed} />
                <DataRow label="配置文件" value={configFileLabel} ok={systemInfo.openclaw_config_exists} />
                <DataRow label="系统环境" value={`${osLabel} (${systemInfo.arch})`} />
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardContent className="p-5">
              <h3 className="text-[13px] font-semibold text-foreground/90 mb-4">如果你只想快速开始</h3>
              <div className="space-y-3">
                <InlineNote
                  tone="neutral"
                  title="最短路径"
                  body="1. 启动网关  2. 配主模型  3. 打开 Control UI。频道这类扩展能力都可以后面再补。"
                />
                {gatewayProbeError && (
                  <InlineNote
                    tone="warn"
                    title="当前网关还有一点异常"
                    body={gatewayProbeError}
                  />
                )}
                {!systemInfo.openclaw_doctor_ok && (
                  <InlineNote
                    tone="warn"
                    title="健康检查没通过时，先别折腾复杂功能"
                    body="先去设置页重新运行 onboard 或检查安装问题，通常比在首页看很多状态更有效。"
                  />
                )}
                {riskCount > 0 && topFinding && (
                  <InlineNote
                    tone="warn"
                    title={topFinding.title ?? "有一个需要注意的提醒"}
                    body={firstMeaningfulLine(topFinding.remediation ?? topFinding.detail) ?? "建议到设置页继续处理。"}
                  />
                )}
              </div>
            </CardContent>
          </Card>
        </div>
          </>
        )}
    </PageShell>
  );
}

function parseJsonResult<T>(result: CommandResult): T | null {
  if (!result.success || !result.stdout.trim()) {
    return null;
  }

  try {
    return JSON.parse(result.stdout) as T;
  } catch {
    return null;
  }
}

function compactMessages(messages: string[]) {
  return messages
    .map((message) => firstMeaningfulLine(message))
    .filter((message): message is string => Boolean(message));
}

function firstMeaningfulLine(text?: string | null) {
  if (!text) {
    return null;
  }

  return text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean) ?? null;
}

function countConfiguredChannelsFromSnapshot(result: CommandResult) {
  if (!result.success || !result.stdout.trim()) {
    return 0;
  }

  let payload: Record<string, unknown> | null = null;
  try {
    payload = JSON.parse(result.stdout) as Record<string, unknown>;
  } catch {
    return 0;
  }

  const chat = payload.chat;
  if (!chat || typeof chat !== "object" || Array.isArray(chat)) {
    return 0;
  }

  let count = 0;
  for (const accounts of Object.values(chat as Record<string, unknown>)) {
    if (!accounts || typeof accounts !== "object" || Array.isArray(accounts)) {
      continue;
    }

    for (const account of Object.values(accounts as Record<string, unknown>)) {
      if (!account || typeof account !== "object" || Array.isArray(account)) {
        continue;
      }
      const enabled = (account as Record<string, unknown>).enabled;
      if (enabled !== false) {
        count += 1;
      }
    }
  }

  return count;
}

function getPathTail(path?: string | null) {
  if (!path) {
    return "未找到";
  }

  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function BeginnerStepCard({ title, description, done, actionLabel, onAction }: SetupStep) {
  return (
    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {done ? (
              <CircleCheck size={15} className="text-emerald-400 shrink-0" />
            ) : (
              <CircleX size={15} className="text-amber-400 shrink-0" />
            )}
            <p className="text-[13px] font-medium text-foreground/90">{title}</p>
          </div>
          <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">{description}</p>
        </div>
        <StatusBadge tone={done ? "good" : "warn"}>
          {done ? "已完成" : "待处理"}
        </StatusBadge>
      </div>

      <AsyncActionButton size="sm" variant={done ? "outline" : "default"} className="mt-4" onAction={onAction}>
        {actionLabel}
        <ArrowRight size={13} />
      </AsyncActionButton>
    </div>
  );
}

function AsyncActionButton({
  onAction,
  children,
  ...buttonProps
}: ComponentProps<typeof Button> & { onAction: () => void | Promise<void> }) {
  const [pending, setPending] = useState(false);

  const handleClick = () => {
    if (pending) {
      return;
    }

    const result = onAction();
    if (isPromiseLike(result)) {
      setPending(true);
      result.finally(() => {
        setPending(false);
      });
    }
  };

  return (
    <Button {...buttonProps} onClick={handleClick} disabled={buttonProps.disabled || pending}>
      {pending && <Loader2 size={13} className="animate-spin" />}
      {children}
    </Button>
  );
}

function DataRow({ label, value, ok }: { label: string; value: string; ok?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div className="flex items-center gap-2 min-w-0">
        {ok !== undefined && (
          <div className={`w-1.5 h-1.5 rounded-full ${ok ? "bg-emerald-500" : "bg-red-500/60"}`} />
        )}
        <span className="text-[12px] text-muted-foreground">{label}</span>
      </div>
      <span className="text-[13px] font-mono text-right text-foreground/80 break-all">{value}</span>
    </div>
  );
}

function ActionLinkCard({ icon: Icon, title, desc, href, onClick }: {
  icon: typeof ExternalLink;
  title: string;
  desc: string;
  href?: string;
  onClick?: () => void | Promise<void>;
}) {
  const [pending, setPending] = useState(false);

  const handleAction = () => {
    if (!onClick || pending) {
      return;
    }

    const result = onClick();
    if (isPromiseLike(result)) {
      setPending(true);
      result.finally(() => {
        setPending(false);
      });
    }
  };

  const inner = (
    <Card className={(href || onClick) ? "hover:border-primary/30 transition-colors cursor-pointer group" : ""}>
      <CardContent className="p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-3 min-w-0">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-white/[0.04] text-muted-foreground group-hover:bg-primary/15 group-hover:text-primary transition-colors">
              <Icon size={16} />
            </div>
            <div className="min-w-0">
              <p className="text-[13px] font-medium text-foreground/90">{title}</p>
              <p className="text-[11px] text-muted-foreground leading-relaxed">{desc}</p>
            </div>
          </div>
          {(href || onClick) && (
            pending
              ? <Loader2 size={14} className="animate-spin text-muted-foreground shrink-0" />
              : <ArrowRight size={14} className="text-muted-foreground group-hover:text-primary transition-colors shrink-0" />
          )}
        </div>
      </CardContent>
    </Card>
  );

  if (onClick) {
    return (
      <button type="button" className="w-full text-left disabled:cursor-not-allowed" onClick={handleAction} disabled={pending}>
        {inner}
      </button>
    );
  }

  if (href) {
    return (
      <a href={href} target="_blank" rel="noreferrer">
        {inner}
      </a>
    );
  }

  return inner;
}

function SnapshotChip({ label, value, tone = "neutral" }: { label: string; value: string; tone?: Tone }) {
  return (
    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] px-3 py-2">
      <div className="text-[10px] uppercase tracking-widest text-muted-foreground">{label}</div>
      <div className={`mt-1 text-[12px] font-medium ${toneToTextClass(tone)}`}>{value}</div>
    </div>
  );
}

function isPromiseLike(value: void | Promise<void>): value is Promise<void> {
  return Boolean(value) && typeof (value as Promise<void>).then === "function";
}

function InlineNote({ title, body, tone, className = "" }: {
  title: string;
  body: string;
  tone: Tone;
  className?: string;
}) {
  const toneClass = tone === "good"
    ? "border-emerald-500/20 bg-emerald-500/5"
    : tone === "warn"
      ? "border-amber-500/20 bg-amber-500/5"
      : tone === "danger"
        ? "border-red-500/20 bg-red-500/5"
        : "border-white/[0.06] bg-white/[0.03]";

  return (
    <div className={`rounded-xl border p-3 ${toneClass} ${className}`}>
      <p className="text-[12px] font-medium text-foreground/90">{title}</p>
      <p className="mt-1 text-[11px] leading-relaxed text-muted-foreground">{body}</p>
    </div>
  );
}

function StatusBadge({ tone, children }: { tone: Tone; children: ReactNode }) {
  const className = tone === "good"
    ? "bg-emerald-500/10 text-emerald-400 border-emerald-500/15"
    : tone === "warn"
      ? "bg-amber-500/10 text-amber-400 border-amber-500/15"
      : tone === "danger"
        ? "bg-red-500/10 text-red-400 border-red-500/15"
        : "bg-white/[0.04] text-muted-foreground border-white/[0.06]";

  return (
    <Badge variant="outline" className={className}>
      {children}
    </Badge>
  );
}

function toneToTextClass(tone: Tone) {
  if (tone === "good") {
    return "text-emerald-400";
  }
  if (tone === "warn") {
    return "text-amber-400";
  }
  if (tone === "danger") {
    return "text-red-400";
  }
  return "text-foreground/90";
}
