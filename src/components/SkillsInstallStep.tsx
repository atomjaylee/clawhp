import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Puzzle, ArrowRight, Loader2, CheckCircle2, XCircle,
  Terminal, SkipForward,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Progress } from "@/components/ui/progress";
import type { LogEntry } from "../types";

interface SkillsInstallStepProps {
  onNext: () => void;
}

interface InstallEvent {
  level: string;
  message: string;
}

export default function SkillsInstallStep({ onNext }: SkillsInstallStepProps) {
  const [installing, setInstalling] = useState(false);
  const [done, setDone] = useState(false);
  const [success, setSuccess] = useState(false);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [progress, setProgress] = useState(0);
  const logEndRef = useRef<HTMLDivElement>(null);

  const addLog = (level: LogEntry["level"], message: string) => {
    setLogs((prev) => [...prev, { timestamp: new Date(), level, message }]);
  };

  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [logs]);

  useEffect(() => {
    const unlisten = listen<InstallEvent>("skill-install-log", (event) => {
      const { level, message } = event.payload;

      if (level === "done") {
        setProgress(100);
        if (message === "success") {
          addLog("success", "所有默认技能安装完成");
          setSuccess(true);
        } else {
          addLog("warn", "部分技能安装完成");
          setSuccess(true); // partial is still ok
        }
        setDone(true);
        setInstalling(false);
        return;
      }

      if (message.trim()) {
        addLog(level === "error" ? "error" : "info", message);
      }

      setProgress((p) => Math.min(90, p + 20));
    });

    return () => { unlisten.then((fn) => fn()); };
  }, []);

  const handleInstall = async () => {
    setInstalling(true);
    setDone(false);
    setSuccess(false);
    setLogs([]);
    setProgress(5);

    addLog("info", "正在安装默认技能包...");

    try {
      await invoke("install_default_skills");
    } catch (e) {
      setProgress(0);
      addLog("error", `安装失败: ${e}`);
      setDone(true);
      setInstalling(false);
      setSuccess(false);
    }
  };

  const defaultSkills = [
    { name: "OpenCode", desc: "代码生成与分析" },
    { name: "Terminal", desc: "终端命令执行" },
    { name: "File Tools", desc: "文件读写操作" },
  ];

  return (
    <div className="flex-1 flex flex-col p-6 animate-fade-in overflow-hidden">
      <div className="mb-4">
        <h2 className="text-lg font-semibold mb-1">安装默认技能</h2>
        <p className="text-[13px] text-muted-foreground">为智能体安装常用技能包，以便开箱即用</p>
      </div>

      {!installing && !done && (
        <div className="mb-4 space-y-2">
          {defaultSkills.map((skill) => (
            <div
              key={skill.name}
              className="flex items-center gap-3 p-3 rounded-xl border border-white/[0.06] bg-white/[0.02]"
            >
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-violet-500/10">
                <Puzzle size={14} className="text-violet-400" />
              </div>
              <div>
                <div className="text-[13px] font-medium">{skill.name}</div>
                <div className="text-[11px] text-muted-foreground">{skill.desc}</div>
              </div>
            </div>
          ))}
        </div>
      )}

      {(installing || logs.length > 0) && (
        <div className="flex-1 flex flex-col min-h-0">
          <div className="mb-2.5">
            <div className="flex items-center justify-between mb-1">
              <span className="text-[13px] text-muted-foreground">
                {installing ? "安装中..." : done && success ? "安装完成" : "安装进度"}
              </span>
              <span className="text-[11px] text-muted-foreground">{Math.round(progress)}%</span>
            </div>
            <Progress value={progress} className="h-1.5" />
          </div>

          <Card className="flex-1 min-h-0 overflow-hidden flex flex-col">
            <div className="flex items-center gap-2 px-3 py-1.5 border-b border-white/[0.06]">
              <Terminal size={12} className="text-muted-foreground" />
              <span className="text-[11px] font-medium text-muted-foreground">安装日志</span>
              {installing && <Loader2 size={11} className="animate-spin text-violet-400 ml-auto" />}
            </div>
            <ScrollArea className="flex-1">
              <div className="p-3 font-mono text-[11px] space-y-0.5">
                {logs.map((log, i) => (
                  <div key={i} className="flex gap-2 leading-5">
                    <span className="text-muted-foreground/40 shrink-0 w-[60px]">
                      {log.timestamp.toLocaleTimeString()}
                    </span>
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
          {done && success && (
            <span className="flex items-center gap-1.5 text-emerald-400">
              <CheckCircle2 size={14} />
              技能安装完成
            </span>
          )}
          {done && !success && (
            <span className="flex items-center gap-1.5 text-red-400">
              <XCircle size={14} />
              安装失败
            </span>
          )}
        </div>
        <div className="flex gap-2">
          {!installing && !done && (
            <>
              <Button variant="ghost" onClick={onNext} className="text-muted-foreground">
                <SkipForward size={14} />
                跳过并进入控制面板
              </Button>
              <Button onClick={handleInstall}>
                安装技能 <ArrowRight size={14} />
              </Button>
            </>
          )}
          {installing && (
            <Button disabled>
              <Loader2 size={14} className="animate-spin" />
              安装中...
            </Button>
          )}
          {done && (
            <Button onClick={onNext}>
              进入控制面板 <ArrowRight size={14} />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
