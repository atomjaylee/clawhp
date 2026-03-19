import { type ReactNode, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Blocks, Download, ExternalLink, FolderOpen, Loader2, PackageOpen,
  Puzzle, RefreshCw, Search, Sparkles, Trash2, Wrench,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import ModuleTabs, { type ModuleTabItem } from "@/components/ui/module-tabs";
import PageShell from "@/components/PageShell";
import type {
  CommandResult,
  OpenClawSkillInfo,
  SkillInfo,
  SkillMarketplaceEntry,
  SkillsRequirementSnapshot,
  SkillRequirementState,
  SkillsDashboardSnapshot,
} from "@/types";

type BundledFilter = "all" | "ready" | "needs-setup";
type SkillsModuleTab = "installed" | "market" | "workspace" | "bundled";

const TENCENT_MARKETPLACE = {
  id: "tencent",
  label: "腾讯 SkillHub",
  description: "只保留腾讯 SkillHub 市场，空白时显示推荐 Skills，搜索时直接走腾讯官方前台正在使用的接口。",
  url: "https://skillhub.tencent.com",
} as const;

export default function SkillsPage() {
  const [snapshot, setSnapshot] = useState<SkillsDashboardSnapshot | null>(null);
  const [pageLoading, setPageLoading] = useState(true);
  const [pageError, setPageError] = useState("");
  const [bundledDialogOpen, setBundledDialogOpen] = useState(false);
  const [bundledQuery, setBundledQuery] = useState("");
  const [marketQuery, setMarketQuery] = useState("");
  const [marketItems, setMarketItems] = useState<SkillMarketplaceEntry[]>([]);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState("");
  const [marketMessage, setMarketMessage] = useState("");
  const [requirementsLoading, setRequirementsLoading] = useState(false);
  const [requirementsLoaded, setRequirementsLoaded] = useState(false);
  const [requirementWarnings, setRequirementWarnings] = useState<string[]>([]);
  const [requirementMessage, setRequirementMessage] = useState("");
  const [requirementError, setRequirementError] = useState("");
  const [installingRequirementKey, setInstallingRequirementKey] = useState<string | null>(null);
  const [installingSlug, setInstallingSlug] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<SkillInfo | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const [deleting, setDeleting] = useState<string | null>(null);
  const [bundledFilter, setBundledFilter] = useState<BundledFilter>("all");
  const [moduleTab, setModuleTab] = useState<SkillsModuleTab>("installed");

  async function fetchSnapshot(options?: { silent?: boolean }) {
    if (!options?.silent) {
      setPageLoading(true);
    }

    setRequirementError("");

    try {
      const nextSnapshot = await invoke<SkillsDashboardSnapshot>("get_skills_dashboard_snapshot");
      setSnapshot(nextSnapshot);
      setPageError("");
      setRequirementWarnings([]);
      setRequirementsLoaded(nextSnapshot.openclawSkills.length === 0);
      return nextSnapshot;
    } catch (error) {
      setPageError(`读取 Skills 状态失败: ${error}`);
      setSnapshot(null);
      setRequirementWarnings([]);
      setRequirementsLoaded(false);
      return null;
    } finally {
      setPageLoading(false);
    }
  }

  async function fetchRequirementDetails() {
    setRequirementsLoading(true);
    setRequirementError("");

    try {
      const payload = await invoke<SkillsRequirementSnapshot>("get_skills_requirement_snapshot");
      setSnapshot((current) => {
        if (!current) {
          return current;
        }

        const mergedSkills = current.openclawSkills.map((skill) => ({
          ...skill,
          installHints: payload.installHintsBySkill[skill.name] ?? [],
        }));

        return {
          ...current,
          openclawSkills: mergedSkills,
          summary: {
            ...current.summary,
            eligibleCount: payload.eligibleCount || mergedSkills.filter((skill) => skill.eligible).length,
            missingRequirementCount:
              payload.missingRequirementCount
              || mergedSkills.filter((skill) => !isRequirementStateEmpty(skill.missing)).length,
          },
        };
      });
      setRequirementWarnings(payload.warnings ?? []);
      setRequirementError("");
      setRequirementsLoaded(true);
    } catch (error) {
      setRequirementWarnings([]);
      setRequirementError(`读取依赖建议失败: ${error}`);
    } finally {
      setRequirementsLoading(false);
    }
  }

  async function loadSkillsView(options?: { silent?: boolean }) {
    const nextSnapshot = await fetchSnapshot(options);
    if (!nextSnapshot || nextSnapshot.openclawSkills.length === 0) {
      return;
    }
    void fetchRequirementDetails();
  }

  async function loadMarketplace(query: string) {
    setMarketLoading(true);
    setMarketError("");

    try {
      const result = await invoke<SkillMarketplaceEntry[]>("search_skill_marketplace", {
        source: TENCENT_MARKETPLACE.id,
        query: query.trim() ? query.trim() : null,
        limit: query.trim() ? 10 : 12,
      });
      setMarketItems(result);
    } catch (error) {
      setMarketItems([]);
      setMarketError(`读取技能市场失败: ${error}`);
    } finally {
      setMarketLoading(false);
    }
  }

  useEffect(() => {
    void loadSkillsView();
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void loadMarketplace(marketQuery);
    }, marketQuery.trim() ? 220 : 0);

    return () => window.clearTimeout(timer);
  }, [marketQuery]);

  async function refreshAll() {
    await Promise.all([
      loadSkillsView({ silent: false }),
      loadMarketplace(marketQuery),
    ]);
  }

  async function handleInstall(slug: string) {
    const trimmedSlug = slug.trim();
    if (!trimmedSlug) {
      return;
    }

    const installSourceLabel = TENCENT_MARKETPLACE.label;
    setInstallingSlug(trimmedSlug);
    setMarketError("");
    setMarketMessage("");

    try {
      const result: CommandResult = await invoke("install_skill_from_marketplace", {
        source: TENCENT_MARKETPLACE.id,
        slug: trimmedSlug,
        force: false,
      });

      if (!result.success) {
        setMarketError(result.stderr || `安装 ${trimmedSlug} 失败`);
        return;
      }

      setMarketMessage(`已从 ${installSourceLabel} 安装 ${trimmedSlug}`);
      await loadSkillsView({ silent: true });
      await loadMarketplace(marketQuery);
    } catch (error) {
      setMarketError(`安装 ${trimmedSlug} 失败: ${error}`);
    } finally {
      setInstallingSlug(null);
    }
  }

  async function handleDelete() {
    if (!pendingDelete) {
      return;
    }

    setDeleting(pendingDelete.name);
    setDeleteError("");

    try {
      const result: CommandResult = await invoke("delete_skill", { name: pendingDelete.name });
      if (!result.success) {
        setDeleteError(result.stderr || `删除 ${pendingDelete.name} 失败`);
        return;
      }

      setPendingDelete(null);
      await loadSkillsView({ silent: true });
      await loadMarketplace(marketQuery);
    } catch (error) {
      setDeleteError(`删除 ${pendingDelete.name} 失败: ${error}`);
    } finally {
      setDeleting(null);
    }
  }

  async function handleInstallRequirement(skill: OpenClawSkillInfo, hintId: string, hintLabel: string) {
    const requirementKey = `${skill.source}:${skill.name}:${hintId}`;
    setInstallingRequirementKey(requirementKey);
    setRequirementError("");
    setRequirementMessage("");

    try {
      const result: CommandResult = await invoke("install_skill_requirement", {
        skillName: skill.name,
        source: skill.source,
        hintId,
      });
      if (!result.success) {
        setRequirementError(result.stderr || `安装 ${hintLabel} 失败`);
        return;
      }

      setRequirementMessage(result.stdout.trim() || `已执行 ${hintLabel}`);
      await loadSkillsView({ silent: true });
    } catch (error) {
      setRequirementError(`安装 ${hintLabel} 失败: ${error}`);
    } finally {
      setInstallingRequirementKey(null);
    }
  }

  const managedSkills = snapshot?.managedSkills ?? [];
  const openclawSkills = snapshot?.openclawSkills ?? [];
  const bundledSkills = openclawSkills.filter((skill) => skill.source === "openclaw-bundled");
  const workspaceSkills = openclawSkills.filter((skill) => skill.source === "openclaw-workspace");
  const readyBundledCount = bundledSkills.filter((skill) => skill.eligible).length;
  const missingBundledCount = bundledSkills.filter((skill) => !skill.eligible).length;
  const warningMessages = [...(snapshot?.warnings ?? []), ...requirementWarnings];
  const managedSkillNames = new Set(managedSkills.map((skill) => skill.originSlug || skill.name));
  const openclawSkillNames = new Set(openclawSkills.map((skill) => skill.name));
  const moduleTabs: ModuleTabItem<SkillsModuleTab>[] = [
    { id: "installed", label: "已安装", icon: Puzzle, badge: managedSkills.length },
    { id: "market", label: "市场", icon: Download, badge: marketItems.length || "推荐" },
    { id: "workspace", label: "工作区", icon: FolderOpen, badge: workspaceSkills.length },
    { id: "bundled", label: "内置能力", icon: Sparkles, badge: bundledSkills.length },
  ];
  const filteredBundledSkills = bundledSkills.filter((skill) => {
    const keyword = bundledQuery.trim().toLowerCase();
    if (bundledFilter === "ready") {
      if (!skill.eligible) {
        return false;
      }
    }
    if (bundledFilter === "needs-setup") {
      if (skill.eligible) {
        return false;
      }
    }
    if (!keyword) {
      return true;
    }
    return [
      skill.name,
      skill.description,
      skill.installHints.map((hint) => hint.label).join(" "),
    ]
      .join(" ")
      .toLowerCase()
      .includes(keyword);
  });
  const canDirectInstall = isLikelySkillSlug(marketQuery)
    && !managedSkillNames.has(marketQuery.trim())
    && !openclawSkillNames.has(marketQuery.trim());

  return (
    <TooltipProvider delayDuration={250}>
      <PageShell
        bodyClassName="space-y-5"
        header={(
          <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
            <div className="flex items-center gap-3">
              <div className="flex h-10 w-10 items-center justify-center rounded-2xl bg-teal-500/12 shadow-lg shadow-teal-500/10">
                <Puzzle size={18} className="text-teal-300" />
              </div>
              <div>
                <h2 className="text-sm font-semibold">Skills 管理</h2>
                <p className="text-[12px] text-muted-foreground">
                  {pageLoading && !snapshot
                    ? "正在读取本地与 OpenClaw Skills 状态"
                    : requirementsLoading && snapshot
                      ? `${managedSkills.length} 个额外安装，${openclawSkills.length} 个 OpenClaw 可用 Skills，依赖建议补充中`
                    : `${managedSkills.length} 个额外安装，${openclawSkills.length} 个 OpenClaw 可用 Skills`}
                </p>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-1.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8"
                    onClick={() => void refreshAll()}
                    disabled={pageLoading || marketLoading || requirementsLoading}
                  >
                    {(pageLoading || marketLoading || requirementsLoading) ? <Loader2 className="animate-spin" /> : <RefreshCw size={14} />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>刷新 Skills 列表与市场</TooltipContent>
              </Tooltip>
              <Button size="sm" variant="outline" asChild>
                <a href="https://docs.openclaw.ai/concepts/skills" target="_blank" rel="noreferrer">
                  <ExternalLink />
                  OpenClaw 文档
                </a>
              </Button>
            </div>
          </div>
        )}
      >
        {pageLoading && !snapshot ? (
          <LoadingState />
        ) : (
          <>
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
              <SummaryCard
                icon={Download}
                title="额外安装"
                value={snapshot?.summary.managedCount ?? 0}
                hint="安装到 ~/.openclaw/skills"
                tone="teal"
              />
              <SummaryCard
                icon={Sparkles}
                title="自带 Skills"
                value={snapshot?.summary.bundledCount ?? 0}
                hint="OpenClaw 自带，无需再装"
                tone="amber"
              />
              <SummaryCard
                icon={FolderOpen}
                title="工作区 Skills"
                value={snapshot?.summary.workspaceCount ?? 0}
                hint="来自工作区透出的能力"
                tone="cyan"
              />
              <SummaryCard
                icon={Wrench}
                title="待补依赖"
                value={snapshot?.summary.missingRequirementCount ?? 0}
                hint={requirementsLoading && !requirementsLoaded ? "正在后台补充安装建议" : "需要额外 CLI、环境变量或频道配置"}
                tone="rose"
              />
            </div>

            {pageError && (
              <NoticeCard title="读取失败" tone="red">
                {pageError}
              </NoticeCard>
            )}

            {warningMessages.length ? (
              <NoticeCard title="部分数据未完全加载" tone="amber">
                {warningMessages.join("；")}
              </NoticeCard>
            ) : null}

            {requirementError && (
              <div className="rounded-xl border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-200">
                {requirementError}
              </div>
            )}

            {requirementMessage && (
              <div className="rounded-xl border border-cyan-500/20 bg-cyan-500/10 px-3 py-2.5 text-[12px] text-cyan-100">
                {requirementMessage}
              </div>
            )}

            <ModuleTabs items={moduleTabs} value={moduleTab} onValueChange={setModuleTab} />

            {moduleTab === "bundled" && (
              <Card className="overflow-hidden border-white/[0.08] bg-[radial-gradient(circle_at_top_left,rgba(8,145,178,0.14),transparent_38%),linear-gradient(180deg,rgba(255,255,255,0.04),rgba(255,255,255,0.02))]">
                <CardContent className="space-y-4 p-4">
                  <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2">
                        <Sparkles size={15} className="text-cyan-300" />
                        <h3 className="text-[14px] font-semibold">OpenClaw 自带 Skills</h3>
                      </div>
                      <p className="text-[12px] text-muted-foreground">
                        自带 Skills 数量很多，这里先保留摘要；需要时再进弹窗集中查看和补依赖。
                      </p>
                    </div>
                    <Button size="sm" onClick={() => setBundledDialogOpen(true)}>
                      <Sparkles size={14} />
                      查看全部
                    </Button>
                  </div>

                  <div className="grid gap-3 md:grid-cols-3">
                    <CompactMetric label="自带总数" value={bundledSkills.length} tone="amber" />
                    <CompactMetric label="可直接用" value={readyBundledCount} tone="teal" />
                    <CompactMetric label="待补依赖" value={missingBundledCount} tone="rose" />
                  </div>

                  <div className="flex flex-wrap gap-2">
                    {bundledSkills.slice(0, 8).map((skill) => (
                      <Badge key={skill.name} className="border-0 bg-white/[0.06] px-2 py-0.5 text-[10px] text-muted-foreground">
                        {skill.name}
                      </Badge>
                    ))}
                    {bundledSkills.length > 8 ? (
                      <Badge className="border-0 bg-cyan-500/12 px-2 py-0.5 text-[10px] text-cyan-100">
                        +{bundledSkills.length - 8} 个
                      </Badge>
                    ) : null}
                  </div>
                </CardContent>
              </Card>
            )}

            {moduleTab === "workspace" && (
              workspaceSkills.length > 0 ? (
                <>
                  <SectionHeader
                    title="工作区 Skills"
                    description={`这些是当前工作区透出的自定义 Skills。工作区目录：${snapshot?.workspaceDir || "~/.openclaw/workspace"}`}
                  />
                  <div className="grid gap-3 lg:grid-cols-2 xl:grid-cols-3">
                    {workspaceSkills.map((skill) => (
                      <AvailableSkillCard
                        key={`${skill.source}:${skill.name}`}
                        skill={skill}
                        requirementDetailsLoading={requirementsLoading && !requirementsLoaded}
                        installingRequirementKey={installingRequirementKey}
                        onInstallRequirement={(hintId, hintLabel) => void handleInstallRequirement(skill, hintId, hintLabel)}
                      />
                    ))}
                  </div>
                </>
              ) : (
                <Card className="border-dashed border-white/[0.08] bg-white/[0.02]">
                  <CardContent className="flex flex-col items-center justify-center py-14 text-center">
                    <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-white/[0.05]">
                      <FolderOpen size={24} className="text-muted-foreground" />
                    </div>
                    <h3 className="text-sm font-medium">当前工作区还没有透出 Skills</h3>
                    <p className="mt-1 max-w-[420px] text-[12px] text-muted-foreground">
                      如果你在工作区里维护了自定义 Skills，它们会在这里集中展示。
                    </p>
                  </CardContent>
                </Card>
              )
            )}

            {moduleTab === "market" && (
              <Card className="overflow-hidden border-white/[0.08] bg-[radial-gradient(circle_at_top_left,rgba(20,184,166,0.12),transparent_40%),linear-gradient(180deg,rgba(255,255,255,0.04),rgba(255,255,255,0.02))]">
                <CardContent className="space-y-4 p-4">
                  <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2">
                        <Blocks size={15} className="text-teal-300" />
                        <h3 className="text-[14px] font-semibold">腾讯 SkillHub 安装</h3>
                      </div>
                      <p className="text-[12px] text-muted-foreground">
                        这里只接腾讯 SkillHub。空白时展示推荐 Skills，输入关键词后直接搜索腾讯市场。
                      </p>
                    </div>
                    <Button size="sm" variant="outline" asChild>
                      <a href={TENCENT_MARKETPLACE.url} target="_blank" rel="noreferrer">
                        <ExternalLink />
                        打开 {TENCENT_MARKETPLACE.label}
                      </a>
                    </Button>
                  </div>

                  <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_auto]">
                    <div className="space-y-2">
                      <label htmlFor="skill-market-search" className="text-[12px] font-medium text-foreground">
                        搜索或输入 slug
                      </label>
                      <div className="relative">
                        <Search size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
                        <Input
                          id="skill-market-search"
                          value={marketQuery}
                          onChange={(event) => setMarketQuery(event.target.value)}
                          placeholder="例如 weather、one-password、语音识别"
                          className="h-10 border-white/[0.08] bg-black/20 pl-9"
                        />
                      </div>
                      <p className="text-[11px] text-muted-foreground">{TENCENT_MARKETPLACE.description}</p>
                    </div>

                    <div className="flex items-end">
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={!canDirectInstall || Boolean(installingSlug)}
                        onClick={() => void handleInstall(marketQuery.trim())}
                      >
                        {installingSlug === marketQuery.trim() ? <Loader2 className="animate-spin" /> : <Download size={14} />}
                        按 slug 安装
                      </Button>
                    </div>
                  </div>

                  {marketError && (
                    <div className="rounded-xl border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-200">
                      {marketError}
                    </div>
                  )}

                  {marketMessage && (
                    <div className="rounded-xl border border-teal-500/20 bg-teal-500/10 px-3 py-2.5 text-[12px] text-teal-100">
                      {marketMessage}
                    </div>
                  )}

                  {marketLoading ? (
                    <div className="flex items-center justify-center rounded-2xl border border-white/[0.06] bg-black/10 py-12 text-[12px] text-muted-foreground">
                      <Loader2 size={15} className="mr-2 animate-spin" />
                      正在加载技能市场...
                    </div>
                  ) : marketItems.length === 0 ? (
                    <div className="rounded-2xl border border-dashed border-white/[0.08] bg-black/10 p-6 text-center">
                      <p className="text-[13px] font-medium">没有找到可展示的 Skills</p>
                      <p className="mt-1 text-[12px] text-muted-foreground">
                        {marketQuery.trim()
                          ? "试试换一个关键词，或者直接输入准确 slug 安装。"
                          : "腾讯 SkillHub 暂时没有返回推荐 Skills，可以直接输入 slug 安装。"}
                      </p>
                    </div>
                  ) : (
                    <div className="grid gap-3 lg:grid-cols-2">
                      {marketItems.map((item) => {
                        const isInstalled = managedSkillNames.has(item.slug);
                        const isBundled = openclawSkillNames.has(item.slug);
                        const disabled = isInstalled || isBundled || Boolean(installingSlug);

                        return (
                          <Card key={`${item.marketplace}:${item.slug}`} className="border-white/[0.08] bg-black/15">
                            <CardContent className="space-y-3 p-4">
                              <div className="flex items-start justify-between gap-3">
                                <div className="min-w-0 space-y-1">
                                  <div className="flex items-center gap-2">
                                    <p className="truncate text-[13px] font-semibold">{item.displayName}</p>
                                    <Badge className="border-0 bg-white/[0.06] px-1.5 py-0 text-[10px] text-muted-foreground">
                                      腾讯 SkillHub
                                    </Badge>
                                  </div>
                                  <p className="font-mono text-[11px] text-muted-foreground">{item.slug}{item.version ? ` · v${item.version}` : ""}</p>
                                </div>
                                <Button
                                  size="sm"
                                  disabled={disabled}
                                  onClick={() => void handleInstall(item.slug)}
                                >
                                  {installingSlug === item.slug ? <Loader2 className="animate-spin" /> : <Download size={14} />}
                                  {isInstalled ? "已安装" : isBundled ? "已内置" : "安装"}
                                </Button>
                              </div>

                              <p className="line-clamp-3 text-[12px] leading-relaxed text-muted-foreground">
                                {item.summary || "这个技能还没有摘要说明。"}
                              </p>

                              <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
                                <span>来源：腾讯 SkillHub</span>
                                <span>{formatRelativeTime(item.updatedAt)}</span>
                              </div>
                            </CardContent>
                          </Card>
                        );
                      })}
                    </div>
                  )}
                </CardContent>
              </Card>
            )}

            {moduleTab === "installed" && (
              <>
                <SectionHeader
                  title="已额外安装 Skills"
                  description={`这些目录位于 ${snapshot?.managedSkillsDir || "~/.openclaw/skills"}，可以删除后重新安装。`}
                />

                {managedSkills.length === 0 ? (
                  <Card className="border-dashed border-white/[0.08] bg-white/[0.02]">
                    <CardContent className="flex flex-col items-center justify-center py-14 text-center">
                      <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-white/[0.05]">
                        <PackageOpen size={24} className="text-muted-foreground" />
                      </div>
                      <h3 className="text-sm font-medium">还没有额外安装的 Skills</h3>
                      <p className="mt-1 max-w-[420px] text-[12px] text-muted-foreground">
                        OpenClaw 自带技能已经可以直接使用；需要第三方扩展时，再从“市场”模块按需安装。
                      </p>
                    </CardContent>
                  </Card>
                ) : (
                  <div className="grid gap-3 lg:grid-cols-2">
                    {managedSkills.map((skill) => (
                      <InstalledSkillCard
                        key={skill.name}
                        skill={skill}
                        deleting={deleting === skill.name}
                        onDelete={() => {
                          setDeleteError("");
                          setPendingDelete(skill);
                        }}
                      />
                    ))}
                  </div>
                )}
              </>
            )}
          </>
        )}

        {bundledDialogOpen && (
          <div
            className="fixed inset-0 z-[115] flex items-center justify-center bg-black/75 px-4 py-6 backdrop-blur-sm"
            onClick={() => {
              if (!installingRequirementKey) {
                setBundledDialogOpen(false);
              }
            }}
          >
            <Card
              className="flex max-h-[90vh] w-full max-w-6xl flex-col border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40"
              onClick={(event) => event.stopPropagation()}
            >
              <CardContent className="flex min-h-0 flex-1 flex-col gap-4 p-5">
                <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                  <div className="space-y-1">
                    <h3 className="text-[14px] font-semibold">OpenClaw 自带 Skills</h3>
                    <p className="text-[12px] text-muted-foreground">
                      在这里集中查看默认 Skills，并对“待补依赖”的项直接补安装。
                    </p>
                  </div>
                  <Button size="sm" variant="outline" onClick={() => setBundledDialogOpen(false)} disabled={Boolean(installingRequirementKey)}>
                    关闭
                  </Button>
                </div>

                {requirementError && (
                  <div className="rounded-xl border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-200">
                    {requirementError}
                  </div>
                )}

                {requirementMessage && (
                  <div className="rounded-xl border border-cyan-500/20 bg-cyan-500/10 px-3 py-2.5 text-[12px] text-cyan-100">
                    {requirementMessage}
                  </div>
                )}

                <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_auto]">
                  <div className="space-y-2">
                    <label htmlFor="bundled-skill-search" className="text-[12px] font-medium text-foreground">
                      搜索自带 Skills
                    </label>
                    <div className="relative">
                      <Search size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
                      <Input
                        id="bundled-skill-search"
                        value={bundledQuery}
                        onChange={(event) => setBundledQuery(event.target.value)}
                        placeholder="例如 1password、github、voice"
                        className="h-10 border-white/[0.08] bg-black/20 pl-9"
                      />
                    </div>
                  </div>

                  <div className="flex flex-wrap gap-2 lg:justify-end">
                    {[
                      { id: "all", label: "全部" },
                      { id: "ready", label: "可直接用" },
                      { id: "needs-setup", label: "待补依赖" },
                    ].map((filter) => (
                      <button
                        key={filter.id}
                        type="button"
                        className={`rounded-full border px-3 py-1.5 text-[12px] transition-colors ${
                          bundledFilter === filter.id
                            ? "border-cyan-400/40 bg-cyan-500/12 text-cyan-100"
                            : "border-white/[0.08] bg-white/[0.03] text-muted-foreground hover:bg-white/[0.06] hover:text-foreground"
                        }`}
                        onClick={() => setBundledFilter(filter.id as BundledFilter)}
                      >
                        {filter.label}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="min-h-0 flex-1 overflow-y-auto pr-1">
                  {filteredBundledSkills.length === 0 ? (
                    <Card className="border-dashed border-white/[0.08] bg-white/[0.02]">
                      <CardContent className="py-10 text-center text-[12px] text-muted-foreground">
                        当前筛选条件下没有匹配的 OpenClaw 自带 Skills。
                      </CardContent>
                    </Card>
                  ) : (
                    <div className="grid gap-3 lg:grid-cols-2 xl:grid-cols-3">
                      {filteredBundledSkills.map((skill) => (
                        <AvailableSkillCard
                          key={`${skill.source}:${skill.name}`}
                          skill={skill}
                          requirementDetailsLoading={requirementsLoading && !requirementsLoaded}
                          installingRequirementKey={installingRequirementKey}
                          onInstallRequirement={(hintId, hintLabel) => void handleInstallRequirement(skill, hintId, hintLabel)}
                        />
                      ))}
                    </div>
                  )}
                </div>
              </CardContent>
            </Card>
          </div>
        )}

        {pendingDelete && (
          <div
            className="fixed inset-0 z-[120] flex items-center justify-center bg-black/75 px-4 backdrop-blur-sm"
            onClick={() => {
              if (!deleting) {
                setPendingDelete(null);
                setDeleteError("");
              }
            }}
          >
            <Card
              className="w-full max-w-md border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40"
              onClick={(event) => event.stopPropagation()}
            >
              <CardContent className="space-y-4 p-5">
                <div className="space-y-1">
                  <h3 className="text-[14px] font-semibold">确认删除 Skill</h3>
                  <p className="text-[12px] text-muted-foreground">
                    确定要删除 `{pendingDelete.name}` 吗？这个操作会移除本地安装目录，下次如需使用需要重新安装。
                  </p>
                </div>

                {deleteError && (
                  <div className="rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                    {deleteError}
                  </div>
                )}

                <div className="flex justify-end gap-2">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={Boolean(deleting)}
                    onClick={() => {
                      setPendingDelete(null);
                      setDeleteError("");
                    }}
                  >
                    取消
                  </Button>
                  <Button size="sm" disabled={Boolean(deleting)} onClick={() => void handleDelete()}>
                    {deleting ? <Loader2 className="animate-spin" /> : <Trash2 size={14} />}
                    {deleting ? "删除中..." : "确认删除"}
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>
        )}
      </PageShell>
    </TooltipProvider>
  );
}

function LoadingState() {
  return (
    <div className="flex items-center justify-center py-20 text-muted-foreground">
      <Loader2 size={18} className="mr-2 animate-spin" />
      <span className="text-[13px]">正在加载 Skills...</span>
    </div>
  );
}

function SummaryCard({
  icon: Icon,
  title,
  value,
  hint,
  tone,
}: {
  icon: any;
  title: string;
  value: number;
  hint: string;
  tone: "teal" | "amber" | "cyan" | "rose";
}) {
  const toneClass = {
    teal: "bg-teal-500/10 text-teal-200 shadow-teal-500/10",
    amber: "bg-amber-500/10 text-amber-200 shadow-amber-500/10",
    cyan: "bg-cyan-500/10 text-cyan-200 shadow-cyan-500/10",
    rose: "bg-rose-500/10 text-rose-200 shadow-rose-500/10",
  }[tone];

  return (
    <Card className="border-white/[0.08] bg-white/[0.03]">
      <CardContent className="flex items-start justify-between gap-3 p-4">
        <div className="space-y-1">
          <p className="text-[12px] text-muted-foreground">{title}</p>
          <p className="text-2xl font-semibold tracking-tight">{value}</p>
          <p className="text-[11px] leading-relaxed text-muted-foreground">{hint}</p>
        </div>
        <div className={`flex h-10 w-10 items-center justify-center rounded-2xl shadow-lg ${toneClass}`}>
          <Icon size={18} />
        </div>
      </CardContent>
    </Card>
  );
}

function CompactMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "teal" | "amber" | "rose";
}) {
  const toneClass = {
    teal: "border-teal-500/15 bg-teal-500/8 text-teal-100",
    amber: "border-amber-500/15 bg-amber-500/8 text-amber-100",
    rose: "border-rose-500/15 bg-rose-500/8 text-rose-100",
  }[tone];

  return (
    <div className={`rounded-2xl border px-4 py-3 ${toneClass}`}>
      <p className="text-[11px] text-current/70">{label}</p>
      <p className="mt-1 text-xl font-semibold tracking-tight">{value}</p>
    </div>
  );
}

function NoticeCard({
  title,
  tone,
  children,
}: {
  title: string;
  tone: "amber" | "red";
  children: ReactNode;
}) {
  const toneClass = tone === "amber"
    ? "border-amber-500/15 bg-amber-500/8 text-amber-100/90"
    : "border-red-500/20 bg-red-500/8 text-red-100/90";

  return (
    <div className={`rounded-2xl border px-4 py-3 ${toneClass}`}>
      <p className="text-[12px] font-semibold">{title}</p>
      <p className="mt-1 text-[12px] leading-relaxed">{children}</p>
    </div>
  );
}

function SectionHeader({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="space-y-1">
      <h3 className="text-[14px] font-semibold">{title}</h3>
      <p className="text-[12px] text-muted-foreground">{description}</p>
    </div>
  );
}

function InstalledSkillCard({
  skill,
  deleting,
  onDelete,
}: {
  skill: SkillInfo;
  deleting: boolean;
  onDelete: () => void;
}) {
  const sourceLabel = resolveManagedSkillSource(skill);
  const resolvedVersion = [skill.installedVersion, skill.version].find((value) => value && value !== "unknown");

  return (
    <Card className="group border-white/[0.08] bg-white/[0.02] transition-colors hover:border-teal-400/20">
      <CardContent className="space-y-3 p-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-1">
            <div className="flex items-center gap-2">
              <p className="truncate text-[13px] font-semibold">{skill.name}</p>
              <Badge className={`border-0 px-1.5 py-0 text-[10px] ${skill.enabled ? "bg-teal-500/15 text-teal-200" : "bg-white/[0.06] text-muted-foreground"}`}>
                {skill.enabled ? "启用" : "禁用"}
              </Badge>
            </div>
            <p className="text-[11px] font-mono text-muted-foreground">
              {resolvedVersion ? `v${resolvedVersion}` : "版本未知"}
            </p>
          </div>

          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                className="h-8 w-8 text-muted-foreground transition-opacity hover:text-destructive group-hover:opacity-100"
                disabled={deleting}
                onClick={onDelete}
              >
                {deleting ? <Loader2 className="animate-spin" /> : <Trash2 size={14} />}
              </Button>
            </TooltipTrigger>
            <TooltipContent>删除这个 Skill</TooltipContent>
          </Tooltip>
        </div>

        <div className="flex flex-wrap gap-2">
          <Badge className="border-0 bg-white/[0.06] px-1.5 py-0 text-[10px] text-muted-foreground">
            {sourceLabel}
          </Badge>
          {skill.originSlug && skill.originSlug !== skill.name ? (
            <Badge className="border-0 bg-white/[0.06] px-1.5 py-0 text-[10px] text-muted-foreground">
              slug: {skill.originSlug}
            </Badge>
          ) : null}
        </div>

        <p className="line-clamp-3 text-[12px] leading-relaxed text-muted-foreground">
          {skill.description || "这个 Skill 没有额外描述。"}
        </p>

        <p className="truncate font-mono text-[10px] text-muted-foreground/70">{skill.path}</p>
      </CardContent>
    </Card>
  );
}

function AvailableSkillCard({
  skill,
  requirementDetailsLoading,
  installingRequirementKey,
  onInstallRequirement,
}: {
  skill: OpenClawSkillInfo;
  requirementDetailsLoading: boolean;
  installingRequirementKey: string | null;
  onInstallRequirement: (hintId: string, hintLabel: string) => void;
}) {
  const requirementTags = collectRequirementTags(skill.missing);
  const actionableInstallHints = skill.installHints.filter((hint) => canInstallHintDirectly(hint.kind));
  const passiveInstallHints = skill.installHints.filter((hint) => !canInstallHintDirectly(hint.kind));

  return (
    <Card className="border-white/[0.08] bg-white/[0.02]">
      <CardContent className="space-y-3 p-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-1">
            <div className="flex items-center gap-2">
              <span className="text-base leading-none">{skill.emoji || "🧩"}</span>
              <p className="truncate text-[13px] font-semibold">{skill.name}</p>
            </div>
            <div className="flex flex-wrap gap-2">
              <Badge className="border-0 bg-white/[0.06] px-1.5 py-0 text-[10px] text-muted-foreground">
                {resolveOpenClawSourceLabel(skill.source)}
              </Badge>
              <Badge className={`border-0 px-1.5 py-0 text-[10px] ${skill.eligible ? "bg-teal-500/15 text-teal-200" : "bg-amber-500/15 text-amber-200"}`}>
                {skill.eligible ? "可直接用" : "待补依赖"}
              </Badge>
              {skill.managedInstalled ? (
                <Badge className="border-0 bg-cyan-500/15 px-1.5 py-0 text-[10px] text-cyan-100">
                  已额外安装
                </Badge>
              ) : null}
            </div>
          </div>

          {skill.homepage ? (
            <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground" asChild>
              <a href={skill.homepage} target="_blank" rel="noreferrer">
                <ExternalLink size={14} />
              </a>
            </Button>
          ) : null}
        </div>

        <p className="line-clamp-3 text-[12px] leading-relaxed text-muted-foreground">
          {skill.description || "这个 Skill 暂无描述。"}
        </p>

        {requirementTags.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {requirementTags.slice(0, 4).map((tag) => (
              <Badge key={tag} className="border-0 bg-amber-500/10 px-1.5 py-0 text-[10px] text-amber-100/90">
                {tag}
              </Badge>
            ))}
          </div>
        ) : null}

        {!skill.eligible && skill.installHints.length === 0 && requirementDetailsLoading ? (
          <p className="text-[11px] text-muted-foreground">
            正在补充安装建议...
          </p>
        ) : null}

        {actionableInstallHints.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {actionableInstallHints.map((hint) => {
              const requirementKey = `${skill.source}:${skill.name}:${hint.id}`;
              const busy = installingRequirementKey === requirementKey;

              return (
                <Button
                  key={hint.id}
                  size="sm"
                  variant="outline"
                  className="h-auto whitespace-normal border-white/[0.08] bg-white/[0.03] py-2 text-left text-[11px] leading-relaxed"
                  disabled={Boolean(installingRequirementKey)}
                  onClick={() => onInstallRequirement(hint.id, hint.label)}
                >
                  {busy ? <Loader2 className="animate-spin" /> : <Wrench size={14} />}
                  {hint.label}
                </Button>
              );
            })}
          </div>
        ) : null}

        {passiveInstallHints.length > 0 ? (
          <p className="text-[11px] text-muted-foreground">
            仍需手动处理：{passiveInstallHints.map((hint) => hint.label).join("、")}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

function resolveManagedSkillSource(skill: SkillInfo) {
  if (!skill.originRegistry) {
    return "手动安装";
  }
  if (skill.originRegistry.includes("skillhub.tencent.com") || skill.originRegistry.includes("lightmake.site")) {
    return "腾讯 SkillHub";
  }
  if (skill.originRegistry.includes("clawhub")) {
    return "ClawHub 官方";
  }
  return skill.originRegistry;
}

function resolveOpenClawSourceLabel(source: string) {
  if (source === "openclaw-bundled") {
    return "OpenClaw 自带";
  }
  if (source === "openclaw-workspace") {
    return "工作区";
  }
  return source;
}

function collectRequirementTags(missing: SkillRequirementState) {
  const tags: string[] = [];
  if (missing.bins.length > 0) {
    tags.push(`缺 CLI: ${missing.bins.slice(0, 2).join(", ")}`);
  }
  if (missing.anyBins.length > 0) {
    tags.push(`缺任一命令组: ${missing.anyBins.slice(0, 2).join(", ")}`);
  }
  if (missing.env.length > 0) {
    tags.push(`缺环境变量: ${missing.env.slice(0, 2).join(", ")}`);
  }
  if (missing.config.length > 0) {
    tags.push(`缺配置: ${missing.config.slice(0, 2).join(", ")}`);
  }
  if (missing.os.length > 0) {
    tags.push(`平台限制: ${missing.os.slice(0, 2).join(", ")}`);
  }
  return tags;
}

function canInstallHintDirectly(kind: string) {
  return ["brew", "go", "node", "uv", "download"].includes(kind.trim().toLowerCase());
}

function isRequirementStateEmpty(missing: SkillRequirementState) {
  return missing.bins.length === 0
    && missing.anyBins.length === 0
    && missing.env.length === 0
    && missing.config.length === 0
    && missing.os.length === 0;
}

function formatRelativeTime(value?: number | null) {
  if (!value) {
    return "最近更新";
  }

  const diff = Date.now() - value;
  const minutes = Math.floor(diff / 60000);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (days > 30) {
    return `${Math.floor(days / 30)} 个月前更新`;
  }
  if (days > 0) {
    return `${days} 天前更新`;
  }
  if (hours > 0) {
    return `${hours} 小时前更新`;
  }
  if (minutes > 0) {
    return `${minutes} 分钟前更新`;
  }
  return "刚刚更新";
}

function isLikelySkillSlug(value: string) {
  return /^[a-z0-9][a-z0-9-_]*$/i.test(value.trim());
}
