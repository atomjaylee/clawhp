import { ArrowRight, Sparkles, Shield, Zap, Globe } from "lucide-react";
import { Button } from "@/components/ui/button";

interface WelcomeStepProps {
  onNext: () => void;
  onSkip: () => void;
}

const features = [
  { icon: Sparkles, title: "AI 智能体", desc: "管理多个 AI 模型和智能体" },
  { icon: Zap, title: "技能扩展", desc: "内置基础 Skills，可再从 ClawHub 扩展" },
  { icon: Globe, title: "多频道接入", desc: "Web、API、CLI 等多种访问方式" },
  { icon: Shield, title: "本地优先", desc: "数据和密钥仅保存在本地" },
];

export default function WelcomeStep({ onNext, onSkip }: WelcomeStepProps) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center p-8 animate-fade-in">
      <div className="max-w-md w-full text-center">
        <div className="mx-auto mb-5 flex h-16 w-16 items-center justify-center rounded-2xl bg-gradient-to-br from-teal-400 to-emerald-500 shadow-lg shadow-teal-500/25">
          <img src="/icon.png" alt="clawHelp" className="size-10 rounded-lg" />
        </div>

        <h1 className="text-2xl font-bold mb-2">欢迎使用 clawHelp</h1>
        <p className="text-[13px] text-muted-foreground mb-8 leading-relaxed">
          clawHelp 是 OpenClaw 的桌面管理客户端，帮助你完成安装、配置和日常管理。<br />
          安装向导将引导你完成环境检测、配置和安装。
        </p>

        <div className="grid grid-cols-2 gap-3 mb-8">
          {features.map((f) => (
            <div
              key={f.title}
              className="flex items-start gap-2.5 p-3 rounded-xl border border-white/[0.06] bg-white/[0.02] text-left"
            >
              <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-teal-500/10">
                <f.icon size={14} className="text-teal-400" />
              </div>
              <div>
                <div className="text-[12px] font-medium">{f.title}</div>
                <div className="text-[11px] text-muted-foreground mt-0.5">{f.desc}</div>
              </div>
            </div>
          ))}
        </div>

        <div className="flex flex-col items-center gap-3">
          <Button size="lg" onClick={onNext} className="w-full max-w-[240px]">
            开始安装 <ArrowRight size={14} />
          </Button>
          <div className="space-y-1 text-center">
            <button
              onClick={onSkip}
              className="text-[12px] text-muted-foreground hover:text-foreground transition-colors"
            >
              已装好了？重新检查现有安装 →
            </button>
            <p className="text-[11px] text-muted-foreground/60">
              如果你刚在终端里装完 OpenClaw，这里会重新识别并直接带你进入控制面板。
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
