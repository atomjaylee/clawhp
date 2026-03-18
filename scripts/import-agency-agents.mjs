#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const SOURCE_ROOT = process.argv[2];

if (!SOURCE_ROOT) {
  console.error("Usage: node scripts/import-agency-agents.mjs <agency-agents-repo-path>");
  process.exit(1);
}

const CATEGORY_LABELS = {
  academic: "学术",
  design: "设计",
  engineering: "工程",
  finance: "金融",
  "game-development": "游戏开发",
  hr: "人力资源",
  legal: "法务",
  marketing: "营销",
  "paid-media": "付费媒体",
  product: "产品",
  "project-management": "项目管理",
  sales: "销售",
  "spatial-computing": "空间计算",
  specialized: "专项",
  strategy: "战略",
  support: "支持",
  "supply-chain": "供应链",
  testing: "测试",
};

const AGENT_DIRS = Object.keys(CATEGORY_LABELS);

const SOUL_HEADER_PATTERNS = [
  /identity/i,
  /身份/,
  /记忆/,
  /communication/i,
  /沟通/,
  /style/i,
  /风格/,
  /critical.?rule/i,
  /关键规则/,
  /rules.?you.?must.?follow/i,
];

function parseFrontmatter(raw) {
  const match = raw.match(/^---\n([\s\S]*?)\n---\n?/);
  if (!match) {
    return { frontmatter: {}, body: raw.trim() };
  }

  const frontmatter = {};
  for (const line of match[1].split("\n")) {
    const separatorIndex = line.indexOf(":");
    if (separatorIndex === -1) {
      continue;
    }
    const key = line.slice(0, separatorIndex).trim();
    const value = line.slice(separatorIndex + 1).trim().replace(/^['"]|['"]$/g, "");
    frontmatter[key] = value;
  }

  return {
    frontmatter,
    body: raw.slice(match[0].length).trim(),
  };
}

function splitOpenClawFiles(body, name, description) {
  let currentTarget = "agents";
  let currentSection = "";
  let soul = "";
  let agents = "";

  const flush = () => {
    if (!currentSection) {
      return;
    }
    if (currentTarget === "soul") {
      soul += currentSection;
    } else {
      agents += currentSection;
    }
    currentSection = "";
  };

  for (const line of body.split("\n")) {
    if (/^##\s+/.test(line)) {
      flush();
      currentTarget = SOUL_HEADER_PATTERNS.some((pattern) => pattern.test(line)) ? "soul" : "agents";
    }
    currentSection += `${line}\n`;
  }
  flush();

  return {
    "SOUL.md": soul.trim(),
    "AGENTS.md": agents.trim(),
    "IDENTITY.md": `# ${name}\n${description}`.trim(),
  };
}

async function main() {
  const presets = [];

  for (const category of AGENT_DIRS) {
    const categoryDir = path.join(SOURCE_ROOT, category);
    let files = [];
    try {
      files = await listMarkdownFiles(categoryDir);
    } catch {
      continue;
    }

    for (const absolutePath of files) {
      const relativePath = path.relative(SOURCE_ROOT, absolutePath).replaceAll(path.sep, "/");
      const raw = await fs.readFile(absolutePath, "utf8");
      const { frontmatter, body } = parseFrontmatter(raw);
      const name = frontmatter.name?.trim();
      const description = frontmatter.description?.trim();
      if (!name || !description) {
        continue;
      }

      const id = path.basename(absolutePath, ".md");
      presets.push({
        id,
        name,
        description,
        category,
        categoryLabel: CATEGORY_LABELS[category] ?? category,
        sourcePath: relativePath,
        files: splitOpenClawFiles(body, name, description),
      });
    }
  }

  presets.sort((left, right) => (
    left.category === right.category
      ? left.name.localeCompare(right.name, "zh-Hans-CN")
      : left.category.localeCompare(right.category)
  ));

  const categories = Array.from(
    presets.reduce((map, preset) => {
      map.set(preset.category, {
        id: preset.category,
        label: preset.categoryLabel,
        count: (map.get(preset.category)?.count ?? 0) + 1,
      });
      return map;
    }, new Map()),
  ).map(([, value]) => value);

  const payload = {
    sourceRepo: "https://github.com/jnMetaCode/agency-agents-zh",
    generatedAt: new Date().toISOString(),
    count: presets.length,
    categories,
    presets,
  };

  const outputDir = path.join(process.cwd(), "src", "data");
  await fs.mkdir(outputDir, { recursive: true });
  const outputPath = path.join(outputDir, "agent-presets.json");
  await fs.writeFile(outputPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");

  console.log(`Wrote ${payload.count} presets to ${outputPath}`);
}

async function listMarkdownFiles(dirPath) {
  const entries = await fs.readdir(dirPath, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const absolutePath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...await listMarkdownFiles(absolutePath));
      continue;
    }
    if (entry.isFile() && entry.name.endsWith(".md")) {
      files.push(absolutePath);
    }
  }

  return files;
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
