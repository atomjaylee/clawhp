import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Puzzle, Trash2, Loader2, ExternalLink, RefreshCw, PackageOpen,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import PageShell from "@/components/PageShell";
import type { SkillInfo, CommandResult } from "@/types";

export default function SkillsPage() {
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [deleting, setDeleting] = useState<string | null>(null);

  const fetchSkills = useCallback(async () => {
    setLoading(true);
    try {
      const list: SkillInfo[] = await invoke("list_skills");
      setSkills(list);
    } catch {
      setSkills([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { fetchSkills(); }, [fetchSkills]);

  const handleDelete = async (name: string) => {
    if (!confirm(`确定删除技能 "${name}" 吗？此操作不可撤销。`)) return;
    setDeleting(name);
    try {
      const r: CommandResult = await invoke("delete_skill", { name });
      if (r.success) {
        setSkills((prev) => prev.filter((s) => s.name !== name));
      }
    } catch { /* ignore */ }
    finally { setDeleting(null); }
  };

  return (
    <TooltipProvider delayDuration={300}>
      <PageShell
        header={(
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-teal-500/10">
                <Puzzle size={15} className="text-teal-400" />
              </div>
              <div>
                <h2 className="text-sm font-semibold">Skills 管理</h2>
                <p className="text-[11px] text-muted-foreground">{loading ? "加载中" : `${skills.length} 个技能`}</p>
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon" className="h-7 w-7" onClick={fetchSkills} disabled={loading}>
                    {loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>刷新</TooltipContent>
              </Tooltip>
              <Button size="sm" variant="outline" asChild>
                <a href="https://clawhub.com" target="_blank" rel="noreferrer">
                  <ExternalLink /> 打开 ClawHub
                </a>
              </Button>
            </div>
          </div>
        )}
      >

          {loading ? (
            <div className="flex items-center justify-center py-20 text-muted-foreground">
              <Loader2 size={18} className="animate-spin mr-2" />
              <span className="text-[13px]">加载中...</span>
            </div>
          ) : skills.length === 0 ? (
            <EmptyState />
          ) : (
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {skills.map((skill) => (
                <SkillCard key={skill.name} skill={skill} deleting={deleting === skill.name} onDelete={() => handleDelete(skill.name)} />
              ))}
            </div>
          )}
      </PageShell>
    </TooltipProvider>
  );
}

function SkillCard({ skill, deleting, onDelete }: { skill: SkillInfo; deleting: boolean; onDelete: () => void }) {
  return (
    <Card className="group hover:border-teal-500/20 transition-colors">
      <CardContent className="p-4">
        <div className="flex items-start justify-between gap-2 mb-2">
          <div className="flex items-center gap-2.5 min-w-0">
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-teal-500/10">
              <Puzzle size={14} className="text-teal-400" />
            </div>
            <div className="min-w-0">
              <p className="text-[13px] font-medium truncate">{skill.name}</p>
              <p className="text-[11px] text-muted-foreground font-mono">v{skill.version}</p>
            </div>
          </div>
          <div className="flex items-center gap-1 shrink-0">
            <Badge className={`text-[10px] h-5 px-1.5 border-0 ${skill.enabled ? "bg-teal-500/15 text-teal-400" : "bg-white/5 text-muted-foreground"}`}>
              {skill.enabled ? "启用" : "禁用"}
            </Badge>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" className="h-6 w-6 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground hover:text-destructive" onClick={onDelete} disabled={deleting}>
                  {deleting ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
                </Button>
              </TooltipTrigger>
              <TooltipContent>删除</TooltipContent>
            </Tooltip>
          </div>
        </div>
        {skill.description && (
          <p className="text-[11px] text-muted-foreground line-clamp-2 mt-1">{skill.description}</p>
        )}
        <p className="text-[10px] text-muted-foreground/50 font-mono mt-2 truncate">{skill.path}</p>
      </CardContent>
    </Card>
  );
}

function EmptyState() {
  return (
    <div className="flex flex-col items-center justify-center py-20 text-center">
      <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-white/[0.04] mb-4">
        <PackageOpen size={24} className="text-muted-foreground" />
      </div>
      <h3 className="text-sm font-medium mb-1">还没有额外安装的 Skills</h3>
      <p className="text-[12px] text-muted-foreground mb-5 max-w-[280px]">
        OpenClaw 已自带一套基础 bundled skills。需要更多扩展时，再去 ClawHub 按需安装即可。
      </p>
      <Button size="sm" asChild>
        <a href="https://clawhub.com" target="_blank" rel="noreferrer">
          <ExternalLink /> 浏览 ClawHub
        </a>
      </Button>
    </div>
  );
}
