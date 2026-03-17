import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Zap, ArrowRight, Loader2,
  CheckCircle2, XCircle, Terminal,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Progress } from "@/components/ui/progress";
import type { InstallConfig, LogEntry, SystemInfo } from "../types";

interface InstallStepProps {
  onNext: () => void;
  onInstalled?: (info: SystemInfo) => void;
  systemInfo: SystemInfo | null;
  config: InstallConfig;
}

interface InstallEvent {
  level: string;
  message: string;
}

export default function InstallStep({ onNext, onInstalled, systemInfo, config }: InstallStepProps) {
  const alreadyInstalled = !!systemInfo?.openclaw_fully_installed;
  const isWindows = systemInfo?.os === "windows";
  const isNpmMirrorInstall = config.installMethod === "npm_mirror";
  const installTitle = isNpmMirrorInstall ? "npm 全局安装（国内镜像）" : "官方安装脚本";

  const [installing, setInstalling] = useState(false);
  const [enteringDashboard, setEnteringDashboard] = useState(false);
  const [done, setDone] = useState(alreadyInstalled);
  const [success, setSuccess] = useState(alreadyInstalled);
  const [logs, setLogs] = useState<LogEntry[]>(
    alreadyInstalled
      ? [{ timestamp: new Date(), level: "success", message: `OpenClaw 已安装 (${systemInfo?.openclaw_version})，可跳过此步骤` }]
      : []
  );
  const [progress, setProgress] = useState(alreadyInstalled ? 100 : 0);
  const logEndRef = useRef<HTMLDivElement>(null);
  const progressRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const addLog = (level: LogEntry["level"], message: string) => {
    setLogs((prev) => [...prev, { timestamp: new Date(), level, message }]);
  };

  useEffect(() => { logEndRef.current?.scrollIntoView({ behavior: "smooth" }); }, [logs]);

  const refreshInstalledInfo = useCallback(async () => {
    try {
      const info = await invoke<SystemInfo>("check_system");
      onInstalled?.(info);
    } catch {
      // ignore
    }
  }, [onInstalled]);

  useEffect(() => {
    const unlisten = listen<InstallEvent>("install-log", (event) => {
      const { level, message } = event.payload;

      if (level === "done") {
        if (progressRef.current) clearInterval(progressRef.current);
        if (message === "success") {
          setProgress(100);
          addLog("success", "安装完成！");
          setSuccess(true);
          void refreshInstalledInfo();
        } else {
          setProgress(0);
          addLog("error", "安装失败，请查看上方日志");
          setSuccess(false);
        }
        setDone(true);
        setInstalling(false);
        return;
      }

      if (message.trim()) {
        addLog(level === "error" ? "error" : "info", message);
      }

      setProgress((p) => {
        if (p < 15) return 15;
        if (p < 90) return p + Math.random() * 2;
        return p;
      });
    });

    return () => { unlisten.then((fn) => fn()); };
  }, [refreshInstalledInfo]);

  const handleInstall = () => {
    setInstalling(true);
    setDone(false);
    setSuccess(false);
    setLogs([]);
    setProgress(5);

    addLog("info", `安装方式: ${installTitle}`);
    addLog("info", `执行: ${installCommand}`);
    if (isNpmMirrorInstall) {
      addLog("info", "npm 安装将使用 npmmirror 国内镜像，不会修改你本机全局 npm registry");
    }
    if (config.apiKey) {
      addLog("info", `API 提供商: ${config.apiProvider}`);
      if (config.apiProvider === "custom" && config.customModelId) {
        addLog("info", `自定义模型: ${config.customModelId}`);
      }
      addLog("info", `网关端口: ${config.gatewayPort}`);
    }
    addLog("info", "安装 CLI 后会继续运行 onboard、安装 daemon，并验证网关和健康状态");
    addLog("info", "正在启动安装进程...");

    progressRef.current = setInterval(() => {
      setProgress((p) => (p < 85 ? p + 0.5 : p));
    }, 2000);

    invoke("run_install_command", {
      apiProvider: config.apiProvider,
      apiKey: config.apiKey || null,
      apiBaseUrl: config.apiBaseUrl || null,
      customModelId: config.customModelId || null,
      gatewayPort: config.gatewayPort,
      installMethod: config.installMethod,
    }).catch((e) => {
      if (progressRef.current) clearInterval(progressRef.current);
      setProgress(0);
      addLog("error", `调用失败: ${e}`);
      setDone(true);
      setInstalling(false);
      setSuccess(false);
    });
  };

  const handleContinue = () => {
    setEnteringDashboard(true);
    onNext();
    void refreshInstalledInfo();
  };

  const installCommand = isWindows
    ? (isNpmMirrorInstall
      ? "npm install -g openclaw --registry=https://registry.npmmirror.com"
      : "iwr -useb https://openclaw.ai/install.ps1 | iex")
    : (isNpmMirrorInstall
      ? "npm install -g openclaw --registry=https://registry.npmmirror.com"
      : "curl -fsSL https://openclaw.ai/install.sh | bash");

  return (
    <div className="flex-1 flex flex-col p-6 animate-fade-in overflow-hidden">
      <div className="mb-4">
        <h2 className="text-lg font-semibold mb-1">安装 OpenClaw</h2>
        <p className="text-[13px] text-muted-foreground">
          {isNpmMirrorInstall
            ? "通过 npm 全局安装 CLI，并使用国内镜像源完成安装；随后继续执行 onboard、daemon 安装与可用性校验"
            : "通过官方安装脚本完成 CLI 安装，并按文档执行 onboard、daemon 安装与可用性校验"}
        </p>
      </div>

      {!installing && !done && (
        <div className="mb-4">
          <div className="p-4 rounded-xl border bg-teal-500/5 border-teal-500/25">
            <div className="flex items-center gap-2 mb-2">
              <Zap size={16} className="text-teal-400" />
              <span className="text-[13px] font-medium">{installTitle}</span>
            </div>
            <p className="text-[11px] text-muted-foreground mb-3">
              {isNpmMirrorInstall
                ? "适合国内网络环境。要求本机已安装 Node.js 和 npm，安装源使用 npmmirror。"
                : "自动检测环境、安装所需依赖（Node.js 等）、安装 CLI 并完成初始配置。"}
            </p>
            <code className="text-[11px] text-muted-foreground/70 bg-white/[0.03] px-2 py-1 rounded block font-mono">{installCommand}</code>
          </div>
        </div>
      )}

      {(installing || logs.length > 0) && (
        <div className="flex-1 flex flex-col min-h-0">
          <div className="mb-2.5">
            <div className="flex items-center justify-between mb-1">
              <span className="text-[13px] text-muted-foreground">
                {installing ? "正在安装..." : done && success ? "安装完成" : "安装进度"}
              </span>
              <span className="text-[11px] text-muted-foreground">{Math.round(progress)}%</span>
            </div>
            <Progress value={progress} className="h-1.5" />
          </div>

          <Card className="flex-1 min-h-0 overflow-hidden flex flex-col">
            <div className="flex items-center gap-2 px-3 py-1.5 border-b border-white/[0.06]">
              <Terminal size={12} className="text-muted-foreground" />
              <span className="text-[11px] font-medium text-muted-foreground">安装日志</span>
              {installing && <Loader2 size={11} className="animate-spin text-teal-400 ml-auto" />}
            </div>
            <ScrollArea className="flex-1">
              <div className="p-3 font-mono text-[11px] space-y-0.5">
                {logs.map((log, i) => (
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
                <div ref={logEndRef} />
              </div>
            </ScrollArea>
          </Card>
        </div>
      )}

      <div className="mt-4 flex items-center justify-between">
        <div className="text-[13px]">
          {done && (success ? (
            <span className="flex items-center gap-1.5 text-emerald-400">
              <CheckCircle2 size={14} />
              {alreadyInstalled ? "已安装" : "安装完成"}
            </span>
          ) : done && (
            <span className="flex items-center gap-1.5 text-red-400">
              <XCircle size={14} />
              安装失败，请检查日志
            </span>
          ))}
        </div>
        <div className="flex gap-2">
          {!installing && !done && (
            <Button onClick={handleInstall}>开始安装 <ArrowRight size={14} /></Button>
          )}
          {installing && (
            <Button disabled>
              <Loader2 size={14} className="animate-spin" />
              安装中...
            </Button>
          )}
          {done && success && (
            <Button onClick={handleContinue} disabled={enteringDashboard}>
              {enteringDashboard ? <Loader2 size={14} className="animate-spin" /> : null}
              {enteringDashboard ? "正在进入..." : "进入控制面板"}
              {!enteringDashboard && <ArrowRight size={14} />}
            </Button>
          )}
          {done && !success && (
            <Button variant="secondary" onClick={() => { setDone(false); setSuccess(false); setLogs([]); setProgress(0); }}>
              重试
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
