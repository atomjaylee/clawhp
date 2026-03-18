# clawHelp

clawHelp 是跨平台 OpenClaw 管理客户端，基于 Tauri 2 + React 19 + TypeScript 构建。

## 技术栈

| 层       | 技术                                                         |
| -------- | ------------------------------------------------------------ |
| 桌面框架 | [Tauri 2](https://tauri.app/) (Rust)                         |
| 前端     | React 19 + TypeScript + Vite 6                               |
| 样式     | Tailwind CSS 3.4                                             |
| 图标     | Lucide React                                                 |
| 打包     | Tauri bundler (macOS .dmg / Windows .msi / Linux .deb/.AppImage) |

## 功能

- **仪表盘** — 系统状态、环境信息、进度指标一览
- **Skills 管理** — 浏览、删除已安装的 OpenClaw 技能
- **Agents 管理** — 查看和管理已配置的 Agent
- **模型管理** — 添加 Provider、同步远端模型、设置主模型
- **设置** — 引导向导、版本更新、配置路径
- **安装向导** — 环境检测、一键安装、初始配置

## 前置要求

- [Node.js](https://nodejs.org/) v22+
- [Rust](https://rustup.rs/) (latest stable)
- 平台相关依赖:
  - **macOS**: Xcode Command Line Tools
  - **Linux**: `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  - **Windows**: [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/), WebView2

## 快速开始

```bash
npm install
npm run tauri dev
```

## 构建

```bash
CI=false npm run tauri build
```

## 项目结构

```
clawhelp/
├── src/                          # React 前端
│   ├── App.tsx                   # 主应用 (路由/布局)
│   ├── main.tsx                  # 入口
│   ├── types.ts                  # TypeScript 类型
│   ├── index.css                 # 全局样式/主题变量
│   ├── lib/utils.ts              # 工具函数
│   ├── hooks/use-mobile.tsx      # 响应式 hook
│   └── components/
│       ├── app-sidebar.tsx       # 侧边导航
│       ├── DashboardContent.tsx  # 仪表盘
│       ├── SkillsPage.tsx        # Skills 管理
│       ├── AgentsPage.tsx        # Agents 管理
│       ├── ModelsPage.tsx        # 模型管理
│       ├── SettingsPage.tsx      # 设置
│       ├── LoadingScreen.tsx     # 启动检测
│       ├── SystemCheckStep.tsx   # 环境检测 (向导)
│       ├── InstallStep.tsx       # 安装 (向导)
│       ├── ConfigureStep.tsx     # 配置 (向导)
│       ├── CompleteStep.tsx      # 完成 (向导)
│       └── ui/                   # 基础 UI 组件
├── src-tauri/                    # Tauri / Rust 后端
│   ├── src/lib.rs                # Rust 命令
│   ├── tauri.conf.json           # Tauri 配置
│   ├── capabilities/             # 权限配置
│   └── Cargo.toml
├── package.json
├── vite.config.ts
└── tailwind.config.js
```

## 许可证

MIT
