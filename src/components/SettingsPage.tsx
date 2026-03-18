import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Wand2, RefreshCw, Loader2, ExternalLink, FolderOpen, ArrowRight,
  Trash2, AlertTriangle, Terminal, CheckCircle2, XCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Progress } from "@/components/ui/progress";
import PageShell from "@/components/PageShell";
import type { SystemInfo, CommandResult, LogEntry } from "@/types";

const nextFrame = () => new Promise<void>((r) => requestAnimationFrame(() => setTimeout(r, 0)));

interface UninstallEvent {
  level: string;
  message: string;
}

interface UpdateEvent {
  level: string;
  message: string;
}

interface UpdateStatusSnapshot {
  update?: {
    root?: string;
    installKind?: string;
    packageManager?: string;
    registry?: {
      latestVersion?: string | null;
      error?: string | null;
    };
  };
  channel?: {
    value?: string;
    source?: string;
    label?: string;
  };
  availability?: {
    available?: boolean;
    hasGitUpdate?: boolean;
    hasRegistryUpdate?: boolean;
    latestVersion?: string | null;
    gitBehind?: number | null;
  };
}

interface GithubReleaseSnapshot {
  tag_name?: string;
  name?: string;
  html_url?: string;
  published_at?: string;
  prerelease?: boolean;
  draft?: boolean;
}

interface Props {
  systemInfo: SystemInfo;
  onSystemInfoRefresh?: (info: SystemInfo) => void;
  onUninstallComplete?: () => void;
}

export default function SettingsPage({ systemInfo, onSystemInfoRefresh, onUninstallComplete }: Props) {
  const openclawHome = systemInfo.openclaw_home_path || "~/.openclaw";
  const joinPath = (...parts: string[]) => {
    const separator = systemInfo.os === "windows" ? "\\" : "/";
    return parts
      .filter(Boolean)
      .map((part, index) => {
        if (index === 0) return part.replace(/[\\/]+$/, "");
        return part.replace(/^[\\/]+|[\\/]+$/g, "");
      })
      .join(separator);
  };
  const openclawLogDir = "系统临时目录/openclaw-gateway.log";

  const [currentVersion, setCurrentVersion] = useState(systemInfo.openclaw_version ?? "未知");
  const [updatePhase, setUpdatePhase] = useState<"idle" | "running" | "done">("idle");
  const [updateLogs, setUpdateLogs] = useState<LogEntry[]>([]);
  const [updateSuccess, setUpdateSuccess] = useState(false);
  const [updateProgress, setUpdateProgress] = useState(0);
  const [updateStatusLoading, setUpdateStatusLoading] = useState(false);
  const [updateStatusError, setUpdateStatusError] = useState("");
  const [updateSnapshot, setUpdateSnapshot] = useState<UpdateStatusSnapshot | null>(null);
  const [githubRelease, setGithubRelease] = useState<GithubReleaseSnapshot | null>(null);
  const [githubReleaseLoading, setGithubReleaseLoading] = useState(false);
  const [githubReleaseError, setGithubReleaseError] = useState("");
  const [openingUpdateTerminal, setOpeningUpdateTerminal] = useState(false);
  const [onboarding, setOnboarding] = useState(false);
  const [onboardOutput, setOnboardOutput] = useState("");
  const updateLogEndRef = useRef<HTMLDivElement>(null);
  const updateProgressRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Uninstall state
  const [uninstallPhase, setUninstallPhase] = useState<"idle" | "confirm" | "running" | "done">("idle");
  const [removeData, setRemoveData] = useState(true);
  const [confirmText, setConfirmText] = useState("");
  const [uninstallLogs, setUninstallLogs] = useState<LogEntry[]>([]);
  const [uninstallSuccess, setUninstallSuccess] = useState(false);
  const [uninstallProgress, setUninstallProgress] = useState(0);
  const uninstallLogEndRef = useRef<HTMLDivElement>(null);
  const uninstallProgressRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const addUninstallLog = (level: LogEntry["level"], message: string) => {
    setUninstallLogs((prev) => [...prev, { timestamp: new Date(), level, message }]);
  };

  const addUpdateLog = useCallback((level: LogEntry["level"], message: string) => {
    setUpdateLogs((prev) => [...prev, { timestamp: new Date(), level, message }]);
  }, []);

  useEffect(() => { updateLogEndRef.current?.scrollIntoView({ behavior: "smooth" }); }, [updateLogs]);
  useEffect(() => { uninstallLogEndRef.current?.scrollIntoView({ behavior: "smooth" }); }, [uninstallLogs]);
  useEffect(() => { setCurrentVersion(systemInfo.openclaw_version ?? "未知"); }, [systemInfo.openclaw_version]);
  useEffect(() => () => {
    if (updateProgressRef.current) clearInterval(updateProgressRef.current);
    if (uninstallProgressRef.current) clearInterval(uninstallProgressRef.current);
  }, []);

  const refreshSystemInfo = useCallback(async () => {
    try {
      const info = await invoke<SystemInfo>("check_system");
      setCurrentVersion(info.openclaw_version ?? "未知");
      onSystemInfoRefresh?.(info);
    } catch {
      // ignore
    }
  }, [onSystemInfoRefresh]);

  const refreshUpdateStatus = useCallback(async () => {
    setUpdateStatusLoading(true);
    setUpdateStatusError("");
    try {
      const result = await invoke<CommandResult>("get_update_status_snapshot");
      const snapshot = parseUpdateStatus(result);
      if (snapshot) {
        setUpdateSnapshot(snapshot);
      } else {
        setUpdateStatusError(result.stderr || "无法读取更新状态");
      }
    } catch (error) {
      setUpdateStatusError(`${error}`);
    } finally {
      setUpdateStatusLoading(false);
    }
  }, []);

  const refreshGithubRelease = useCallback(async () => {
    setGithubReleaseLoading(true);
    setGithubReleaseError("");
    try {
      const result = await invoke<CommandResult>("get_github_release_snapshot");
      const snapshot = parseGithubRelease(result);
      if (snapshot) {
        setGithubRelease(snapshot);
      } else {
        setGithubRelease(null);
        setGithubReleaseError(result.stderr || "无法读取 GitHub Releases");
      }
    } catch (error) {
      setGithubRelease(null);
      setGithubReleaseError(`${error}`);
    } finally {
      setGithubReleaseLoading(false);
    }
  }, []);

  const refreshUpdateSignals = useCallback(async () => {
    await Promise.allSettled([refreshUpdateStatus(), refreshGithubRelease()]);
  }, [refreshGithubRelease, refreshUpdateStatus]);

  useEffect(() => {
    void refreshUpdateSignals();
  }, [refreshUpdateSignals]);

  useEffect(() => {
    const unlisten = listen<UpdateEvent>("update-log", (event) => {
      const { level, message } = event.payload;
      if (level === "done") {
        if (updateProgressRef.current) clearInterval(updateProgressRef.current);
        const ok = message === "success";
        setUpdateProgress(100);
        addUpdateLog(ok ? "success" : "error", ok ? "更新完成" : "更新失败，请查看日志");
        setUpdateSuccess(ok);
        setUpdatePhase("done");
        if (ok) {
          void refreshSystemInfo();
          void refreshUpdateSignals();
        }
        return;
      }

      if (message.trim()) {
        const logLevel = level === "error" ? "error" : level === "warn" ? "warn" : "info";
        addUpdateLog(logLevel, message);
      }

      setUpdateProgress((progress) => (progress < 92 ? progress + Math.random() * 3 : progress));
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addUpdateLog, refreshSystemInfo, refreshUpdateSignals]);

  useEffect(() => {
    const unlisten = listen<UninstallEvent>("uninstall-log", (event) => {
      const { level, message } = event.payload;
      if (level === "done") {
        if (uninstallProgressRef.current) clearInterval(uninstallProgressRef.current);
        const ok = message === "success";
        setUninstallProgress(100);
        addUninstallLog(ok ? "success" : "warn", ok ? "卸载完成" : "部分卸载完成，请查看残留项");
        setUninstallSuccess(ok);
        setUninstallPhase("done");
        return;
      }
      if (message.trim()) {
        const logLevel = level === "error" ? "error" : level === "warn" ? "warn" : "info";
        addUninstallLog(logLevel, message);
      }
      setUninstallProgress((p) => (p < 90 ? p + Math.random() * 3 : p));
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  const handleUpdate = async () => {
    if (!githubHasNewRelease) {
      addUpdateLog("warn", "GitHub Releases 暂未检测到比当前更高的新版本，本次不显示更新入口。");
      return;
    }

    setUpdatePhase("running");
    setUpdateLogs([]);
    setUpdateSuccess(false);
    setUpdateProgress(5);
    setUpdateStatusError("");

    addUpdateLog("info", "开始准备更新任务...");
    addUpdateLog("info", "这次会先读取 stable 渠道状态，再以 non-interactive 模式执行更新。");
    addUpdateLog("info", "如果当前环境限制了后台更新，也可以改用下面的“在终端更新”按钮。");

    updateProgressRef.current = setInterval(() => {
      setUpdateProgress((progress) => (progress < 88 ? progress + 0.8 : progress));
    }, 1500);

    invoke("run_update_command").catch((error) => {
      if (updateProgressRef.current) clearInterval(updateProgressRef.current);
      setUpdateProgress(0);
      addUpdateLog("error", `调用失败: ${error}`);
      setUpdatePhase("done");
      setUpdateSuccess(false);
    });
  };

  const handleOpenUpdateTerminal = async () => {
    if (!githubHasNewRelease) {
      addUpdateLog("warn", "GitHub Releases 暂未检测到新版本，已跳过终端更新入口。");
      return;
    }

    setOpeningUpdateTerminal(true);
    try {
      const result = await invoke<CommandResult>("open_update_terminal");
      if (result.success) {
        addUpdateLog("info", result.stdout || "已在外部终端打开更新命令");
        addUpdateLog("info", "你可以在终端里观察完整输出；更新完成后回到这里点“检查状态”刷新版本。");
      } else {
        addUpdateLog("warn", result.stderr || "无法打开外部终端");
      }
    } catch (error) {
      addUpdateLog("warn", `无法打开外部终端: ${error}`);
    } finally {
      setOpeningUpdateTerminal(false);
    }
  };

  const handleStartUninstall = () => {
    setUninstallPhase("running");
    setUninstallLogs([]);
    setUninstallSuccess(false);
    setUninstallProgress(5);
    setConfirmText("");

    addUninstallLog(
      "info",
      `开始卸载 OpenClaw${removeData ? "（含配置与数据目录）" : "（保留数据目录与现有配置）"}`,
    );

    uninstallProgressRef.current = setInterval(() => {
      setUninstallProgress((p) => (p < 85 ? p + 1 : p));
    }, 1500);

    invoke("run_uninstall_command", { removeData }).catch((e) => {
      if (uninstallProgressRef.current) clearInterval(uninstallProgressRef.current);
      setUninstallProgress(0);
      addUninstallLog("error", `调用失败: ${e}`);
      setUninstallPhase("done");
      setUninstallSuccess(false);
    });
  };

  const handleOnboard = async () => {
    setOnboarding(true); setOnboardOutput(""); await nextFrame();
    try {
      const r: CommandResult = await invoke("run_onboard");
      setOnboardOutput(r.stdout || r.stderr || (r.success ? "完成" : "失败"));
    } catch (e) { setOnboardOutput(`${e}`); }
    finally { setOnboarding(false); }
  };

  const currentComparableVersion = extractComparableVersion(currentVersion);
  const githubComparableVersion = extractComparableVersion(
    githubRelease?.tag_name ?? githubRelease?.name ?? null,
  );
  const githubComparison = compareVersionStrings(githubComparableVersion, currentComparableVersion);
  const githubHasNewRelease = githubComparison !== null && githubComparison > 0;
  const hasComparableVersions = githubComparison !== null;
  const latestVisibleVersion = githubRelease?.tag_name
    ?? githubRelease?.name
    ?? updateSnapshot?.availability?.latestVersion
    ?? updateSnapshot?.update?.registry?.latestVersion
    ?? "未知";
  const updateStatusNote = githubReleaseError
    ? `GitHub Releases 检查失败：${pickOneLine(githubReleaseError)}`
    : githubReleaseLoading
      ? "正在对比 GitHub Releases 最新版本..."
      : githubHasNewRelease
        ? `GitHub Releases 已发布新版本 ${latestVisibleVersion}，可以继续更新。`
        : hasComparableVersions
          ? "当前已经是 GitHub Releases 最新稳定版本，不显示更新按钮。"
          : "暂时无法把当前版本和 GitHub Releases 做可靠比对，先不显示更新按钮。";
  const updateSourceNote = updateStatusError
    ? `更新源状态读取失败：${updateStatusError}`
    : updateSnapshot?.update?.registry?.error
      ? `更新源暂时不可达：${pickOneLine(updateSnapshot.update.registry.error)}`
      : githubHasNewRelease && updateSnapshot?.availability?.available === false
        ? "GitHub 已有新版本，但当前更新源还没明确返回可用更新，可能需要稍后再试。"
        : null;

  return (
    <PageShell
      header={(
        <div className="flex items-center justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold">设置与维护</h2>
            <p className="text-[11px] text-muted-foreground">
              当前版本 {currentVersion}，集中管理更新、路径和卸载操作。
            </p>
          </div>
          <Button size="sm" variant="outline" onClick={() => { void refreshSystemInfo(); void refreshUpdateSignals(); }} disabled={updateStatusLoading || githubReleaseLoading || updatePhase === "running" || uninstallPhase === "running"}>
            {(updateStatusLoading || githubReleaseLoading) ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {(updateStatusLoading || githubReleaseLoading) ? "检查中..." : "刷新状态"}
          </Button>
        </div>
      )}
    >
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {/* Onboard */}
          <Card>
            <CardContent className="p-5">
              <div className="flex items-center gap-2.5 mb-3">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-teal-500/10">
                  <Wand2 size={15} className="text-teal-400" />
                </div>
                <div>
                  <h3 className="text-[13px] font-semibold">引导向导</h3>
                  <p className="text-[11px] text-muted-foreground">配置 Auth、网关和后台服务</p>
                </div>
              </div>
              <Button size="sm" onClick={handleOnboard} disabled={onboarding} className="mb-3">
                {onboarding ? <Loader2 className="animate-spin" /> : <Wand2 />}
                {onboarding ? "运行中..." : "openclaw onboard"}
              </Button>
              {onboardOutput && (
                <pre className="rounded-lg bg-white/[0.03] border border-white/[0.06] p-3 text-[11px] font-mono text-muted-foreground whitespace-pre-wrap max-h-40 overflow-auto">{onboardOutput}</pre>
              )}
            </CardContent>
          </Card>

          {/* Update */}
          <Card>
            <CardContent className="p-5">
              <div className="flex items-start justify-between gap-3 mb-4">
                <div className="flex items-center gap-2.5">
                  <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/10">
                    <RefreshCw size={15} className="text-sky-400" />
                  </div>
                  <div>
                    <h3 className="text-[13px] font-semibold">版本更新</h3>
                    <p className="text-[11px] text-muted-foreground">
                      不再直接阻塞界面，先检查状态，再流式显示更新日志。
                    </p>
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={() => void refreshUpdateSignals()} disabled={updateStatusLoading || githubReleaseLoading || updatePhase === "running"}>
                  {updateStatusLoading || githubReleaseLoading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
                  {updateStatusLoading || githubReleaseLoading ? "检查中..." : "检查状态"}
                </Button>
              </div>

              <div className="grid grid-cols-2 gap-2 mb-4">
                <MetricCard label="当前版本" value={currentVersion} tone="good" />
                <MetricCard
                  label="GitHub 最新版"
                  value={latestVisibleVersion}
                  tone={githubHasNewRelease ? "warn" : "neutral"}
                />
                <MetricCard label="更新渠道" value={updateSnapshot?.channel?.label ?? "stable (default)"} />
                <MetricCard
                  label="安装来源"
                  value={
                    [updateSnapshot?.update?.installKind, updateSnapshot?.update?.packageManager]
                      .filter(Boolean)
                      .join(" / ") || "未识别"
                  }
                />
              </div>

              <div className="rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2.5 text-[11px] text-muted-foreground">
                {updateStatusNote}
              </div>

              {updateSourceNote && (
                <div className="mt-3 rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2.5 text-[11px] text-muted-foreground">
                  {updateSourceNote}
                </div>
              )}

              {updateSnapshot?.update?.root && (
                <div className="mt-3 rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2 text-[10px] text-muted-foreground">
                  安装根目录：
                  <code className="ml-1 font-mono text-foreground/70">{updateSnapshot.update.root}</code>
                </div>
              )}

              {githubHasNewRelease && (
                <div className="mt-4 flex flex-wrap gap-2">
                  <Button size="sm" onClick={handleUpdate} disabled={updatePhase === "running"}>
                    {updatePhase === "running" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
                    {updatePhase === "running" ? "更新进行中..." : "开始更新"}
                  </Button>
                  <Button size="sm" variant="outline" onClick={handleOpenUpdateTerminal} disabled={openingUpdateTerminal || updatePhase === "running"}>
                    {openingUpdateTerminal ? <Loader2 className="animate-spin" /> : <Terminal />}
                    {openingUpdateTerminal ? "正在打开终端..." : "在终端更新"}
                  </Button>
                  {githubRelease?.html_url && (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => window.open(githubRelease.html_url, "_blank", "noopener,noreferrer")}
                    >
                      <ExternalLink />
                      查看 Release
                    </Button>
                  )}
                </div>
              )}

              {(updatePhase !== "idle" || updateLogs.length > 0) && (
                <div className="space-y-3 mt-4">
                  <div>
                    <div className="flex items-center justify-between mb-1">
                      <span className="text-[12px] text-muted-foreground">
                        {updatePhase === "running" ? "正在更新..." : updateSuccess ? "更新完成" : "最近一次更新记录"}
                      </span>
                      <span className="text-[11px] text-muted-foreground">{Math.round(updateProgress)}%</span>
                    </div>
                    <Progress value={updateProgress} className="h-1.5" />
                  </div>

                  <Card className="overflow-hidden">
                    <div className="flex items-center gap-2 px-3 py-1.5 border-b border-white/[0.06]">
                      <Terminal size={12} className="text-muted-foreground" />
                      <span className="text-[11px] font-medium text-muted-foreground">更新日志</span>
                      {updatePhase === "running" && <Loader2 size={11} className="animate-spin text-sky-400 ml-auto" />}
                    </div>
                    <ScrollArea className="max-h-48">
                      <div className="p-3 font-mono text-[11px] space-y-0.5">
                        {updateLogs.map((log, i) => (
                          <div key={i} className="flex gap-2 leading-5">
                            <span className="text-muted-foreground/40 shrink-0 w-[60px]">{log.timestamp.toLocaleTimeString()}</span>
                            <span className={
                              log.level === "error" ? "text-red-400"
                                : log.level === "success" ? "text-emerald-400"
                                : log.level === "warn" ? "text-amber-400"
                                : "text-foreground/60"
                            }>{log.message}</span>
                          </div>
                        ))}
                        <div ref={updateLogEndRef} />
                      </div>
                    </ScrollArea>
                  </Card>

                  {updatePhase === "done" && (
                    <div className="flex items-center justify-between">
                      <span className={`flex items-center gap-1.5 text-[13px] ${updateSuccess ? "text-emerald-400" : "text-amber-400"}`}>
                        {updateSuccess ? <CheckCircle2 size={14} /> : <XCircle size={14} />}
                        {updateSuccess ? "OpenClaw 更新完成" : "更新失败或未完成，请查看日志"}
                      </span>
                      <div className="flex gap-2">
                        {!updateSuccess && (
                          <Button size="sm" variant="outline" onClick={() => { setUpdatePhase("idle"); setUpdateLogs([]); setUpdateProgress(0); }}>
                            清空记录
                          </Button>
                        )}
                        <Button size="sm" variant="outline" onClick={() => { void refreshSystemInfo(); void refreshUpdateSignals(); }}>
                          刷新版本
                        </Button>
                      </div>
                    </div>
                  )}
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* Config Paths */}
        <Card>
          <CardContent className="p-5">
            <div className="flex items-center gap-2.5 mb-4">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-amber-500/10">
                <FolderOpen size={15} className="text-amber-400" />
              </div>
              <h3 className="text-[13px] font-semibold">配置路径</h3>
            </div>
            <div className="space-y-2.5">
              {([
                ["安装目录", openclawHome],
                ["配置文件", systemInfo.openclaw_config_path ?? joinPath(openclawHome, "openclaw.json")],
                ["Skills 目录", joinPath(openclawHome, "skills")],
                ["Agents 目录", joinPath(openclawHome, "agents")],
                ["工作空间", joinPath(openclawHome, "workspace")],
                ["凭证", joinPath(openclawHome, "credentials")],
                ["日志", openclawLogDir],
              ] as const).map(([l, p]) => (
                <div key={p} className="flex justify-between items-center">
                  <span className="text-[12px] text-muted-foreground">{l}</span>
                  <code className="font-mono text-[11px] text-foreground/70 bg-white/[0.03] px-2 py-0.5 rounded">{p}</code>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>

        {/* Links */}
        <div className="grid grid-cols-2 gap-4">
          <a href="https://docs.openclaw.ai/start/getting-started" target="_blank" rel="noreferrer">
            <Card className="hover:border-primary/30 transition-colors cursor-pointer group">
              <CardContent className="p-4">
                <div className="flex items-center justify-between">
                  <div>
                    <p className="text-[13px] font-medium">入门指南</p>
                    <p className="text-[11px] text-muted-foreground font-mono mt-0.5">docs.openclaw.ai</p>
                  </div>
                  <ArrowRight size={14} className="text-muted-foreground group-hover:text-primary transition-colors" />
                </div>
              </CardContent>
            </Card>
          </a>
          <a href="https://docs.openclaw.ai/gateway/configuration" target="_blank" rel="noreferrer">
            <Card className="hover:border-primary/30 transition-colors cursor-pointer group">
              <CardContent className="p-4">
                <div className="flex items-center justify-between">
                  <div>
                    <p className="text-[13px] font-medium">网关配置参考</p>
                    <p className="text-[11px] text-muted-foreground font-mono mt-0.5">Gateway configuration</p>
                  </div>
                  <ArrowRight size={14} className="text-muted-foreground group-hover:text-primary transition-colors" />
                </div>
              </CardContent>
            </Card>
          </a>
        </div>

        {/* Uninstall / Danger Zone */}
        <Card className="border-red-500/20">
          <CardContent className="p-5">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-red-500/10">
                  <Trash2 size={15} className="text-red-400" />
                </div>
                <div>
                  <h3 className="text-[13px] font-semibold text-red-400">卸载 OpenClaw</h3>
                  <p className="text-[11px] text-muted-foreground">停止服务、移除 CLI 和数据</p>
                </div>
              </div>
              {uninstallPhase === "idle" && (
                <Button size="sm" variant="outline" className="border-red-500/25 text-red-400 hover:bg-red-500/10 hover:text-red-400" onClick={() => setUninstallPhase("confirm")}>
                  <Trash2 />
                  卸载
                </Button>
              )}
            </div>

            {uninstallPhase === "confirm" && (
              <div className="space-y-3 mt-4">
                <div className="flex items-start gap-2.5 p-3 rounded-lg bg-red-500/5 border border-red-500/15">
                  <AlertTriangle size={16} className="text-red-400 mt-0.5 shrink-0" />
                  <div className="text-[12px] text-red-300/80 space-y-1.5">
                    <p className="font-medium text-red-400">此操作不可撤销</p>
                    <p>将执行以下步骤：</p>
                    <ul className="list-disc list-inside space-y-0.5 text-[11px] text-muted-foreground">
                      <li>停止 OpenClaw 网关服务</li>
                      <li>卸载网关守护进程（launchd / systemd）</li>
                      <li>移除 OpenClaw CLI（npm / pnpm）</li>
                      {removeData && <li>删除 OpenClaw 数据目录、配置文件和备份（如 {openclawHome}、openclaw.json、openclaw.json.bak）</li>}
                      {!removeData && <li>保留现有模型、Provider 和其它用户配置，后续重装会继续沿用</li>}
                    </ul>
                  </div>
                </div>
                <label className="flex items-center gap-2 cursor-pointer">
                  <input type="checkbox" checked={removeData} onChange={(e) => setRemoveData(e.target.checked)} className="rounded border-white/20 bg-white/5 accent-red-500" />
                  <span className="text-[12px] text-muted-foreground">
                    同时删除 OpenClaw 全部本地数据与配置（含配置备份）
                  </span>
                </label>
                <div>
                  <label className="text-[11px] text-muted-foreground block mb-1">
                    请输入 <code className="font-mono text-red-400 bg-red-500/10 px-1 rounded">UNINSTALL</code> 确认
                  </label>
                  <input
                    type="text" value={confirmText} onChange={(e) => setConfirmText(e.target.value)}
                    placeholder="UNINSTALL"
                    className="w-48 bg-white/[0.03] border border-white/[0.08] rounded-lg px-2.5 py-1.5 text-[13px] font-mono placeholder-muted-foreground/30 focus:outline-none focus:border-red-500/40 focus:ring-1 focus:ring-red-500/20 transition-all text-foreground"
                  />
                </div>
                <div className="flex gap-2">
                  <Button size="sm" variant="outline" onClick={() => { setUninstallPhase("idle"); setConfirmText(""); }}>
                    取消
                  </Button>
                  <Button size="sm" disabled={confirmText !== "UNINSTALL"} className="bg-red-600 hover:bg-red-700 text-white" onClick={handleStartUninstall}>
                    <Trash2 />
                    确认卸载
                  </Button>
                </div>
              </div>
            )}

            {(uninstallPhase === "running" || uninstallPhase === "done") && (
              <div className="space-y-3 mt-4">
                <div>
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-[12px] text-muted-foreground">
                      {uninstallPhase === "running" ? "正在卸载..." : uninstallSuccess ? "卸载完成" : "卸载完成（部分）"}
                    </span>
                    <span className="text-[11px] text-muted-foreground">{Math.round(uninstallProgress)}%</span>
                  </div>
                  <Progress value={uninstallProgress} className="h-1.5" />
                </div>

                <Card className="overflow-hidden">
                  <div className="flex items-center gap-2 px-3 py-1.5 border-b border-white/[0.06]">
                    <Terminal size={12} className="text-muted-foreground" />
                    <span className="text-[11px] font-medium text-muted-foreground">卸载日志</span>
                    {uninstallPhase === "running" && <Loader2 size={11} className="animate-spin text-red-400 ml-auto" />}
                  </div>
                  <ScrollArea className="max-h-48">
                    <div className="p-3 font-mono text-[11px] space-y-0.5">
                      {uninstallLogs.map((log, i) => (
                        <div key={i} className="flex gap-2 leading-5">
                          <span className="text-muted-foreground/40 shrink-0 w-[60px]">{log.timestamp.toLocaleTimeString()}</span>
                          <span className={
                            log.level === "error" ? "text-red-400"
                              : log.level === "success" ? "text-emerald-400"
                              : log.level === "warn" ? "text-amber-400"
                              : "text-foreground/60"
                          }>{log.message}</span>
                        </div>
                      ))}
                      <div ref={uninstallLogEndRef} />
                    </div>
                  </ScrollArea>
                </Card>

                {uninstallPhase === "done" && (
                  <div className="flex items-center justify-between">
                    <span className={`flex items-center gap-1.5 text-[13px] ${uninstallSuccess ? "text-emerald-400" : "text-amber-400"}`}>
                      {uninstallSuccess ? <CheckCircle2 size={14} /> : <XCircle size={14} />}
                      {uninstallSuccess ? "OpenClaw 已完全卸载" : "部分卸载，请查看下方残留项"}
                    </span>
                    <div className="flex gap-2">
                      {!uninstallSuccess && (
                        <Button size="sm" variant="outline" onClick={() => { setUninstallPhase("idle"); setUninstallLogs([]); setUninstallProgress(0); }}>
                          重试
                        </Button>
                      )}
                      {uninstallSuccess && onUninstallComplete && (
                        <Button size="sm" onClick={onUninstallComplete}>
                          重新检测
                          <ArrowRight size={14} />
                        </Button>
                      )}
                    </div>
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>
    </PageShell>
  );
}

function parseUpdateStatus(result: CommandResult): UpdateStatusSnapshot | null {
  if (!result.success || !result.stdout.trim()) {
    return null;
  }

  try {
    return JSON.parse(result.stdout) as UpdateStatusSnapshot;
  } catch {
    return null;
  }
}

function parseGithubRelease(result: CommandResult): GithubReleaseSnapshot | null {
  if (!result.success || !result.stdout.trim()) {
    return null;
  }

  try {
    return JSON.parse(result.stdout) as GithubReleaseSnapshot;
  } catch {
    return null;
  }
}

function extractComparableVersion(value?: string | null) {
  if (!value) {
    return null;
  }

  const line = pickOneLine(value);
  const match = line.match(/v?(\d+(?:\.\d+)+(?:-[0-9A-Za-z.-]+)?)/i);
  return match ? match[1].toLowerCase() : null;
}

function compareVersionStrings(left?: string | null, right?: string | null) {
  if (!left || !right) {
    return null;
  }

  const leftParsed = parseVersionParts(left);
  const rightParsed = parseVersionParts(right);
  const maxLength = Math.max(leftParsed.main.length, rightParsed.main.length);

  for (let index = 0; index < maxLength; index += 1) {
    const leftValue = leftParsed.main[index] ?? 0;
    const rightValue = rightParsed.main[index] ?? 0;

    if (leftValue !== rightValue) {
      return leftValue > rightValue ? 1 : -1;
    }
  }

  if (leftParsed.pre.length === 0 && rightParsed.pre.length === 0) {
    return 0;
  }

  if (leftParsed.pre.length === 0) {
    return 1;
  }

  if (rightParsed.pre.length === 0) {
    return -1;
  }

  const preLength = Math.max(leftParsed.pre.length, rightParsed.pre.length);
  for (let index = 0; index < preLength; index += 1) {
    const leftPart = leftParsed.pre[index];
    const rightPart = rightParsed.pre[index];

    if (leftPart === undefined) {
      return -1;
    }

    if (rightPart === undefined) {
      return 1;
    }

    const leftNumber = Number(leftPart);
    const rightNumber = Number(rightPart);
    const bothNumeric = Number.isFinite(leftNumber) && Number.isFinite(rightNumber);

    if (bothNumeric && leftNumber !== rightNumber) {
      return leftNumber > rightNumber ? 1 : -1;
    }

    if (!bothNumeric && leftPart !== rightPart) {
      return leftPart > rightPart ? 1 : -1;
    }
  }

  return 0;
}

function parseVersionParts(version: string) {
  const normalized = version.replace(/^v/i, "").trim().toLowerCase();
  const [main, pre = ""] = normalized.split("-", 2);
  return {
    main: main.split(".").map((part) => Number.parseInt(part, 10) || 0),
    pre: pre ? pre.split(".") : [],
  };
}

function pickOneLine(message?: string | null) {
  if (!message) {
    return "未知错误";
  }

  return message
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean) ?? message;
}

function MetricCard({
  label,
  value,
  tone = "neutral",
}: {
  label: string;
  value: string;
  tone?: "neutral" | "good" | "warn";
}) {
  const toneClass = tone === "good"
    ? "text-emerald-400"
    : tone === "warn"
      ? "text-amber-400"
      : "text-foreground/80";

  return (
    <div className="rounded-lg border border-white/[0.06] bg-white/[0.02] px-3 py-2.5">
      <div className="text-[10px] uppercase tracking-widest text-muted-foreground">{label}</div>
      <div className={`mt-1 text-[12px] font-medium break-all ${toneClass}`}>{value}</div>
    </div>
  );
}
