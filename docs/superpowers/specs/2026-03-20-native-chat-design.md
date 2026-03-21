# clawHelp Native Chat Design

## Goal

在 `clawHelp` 应用内实现一个高性能、应用原生的实时聊天页，直接连接 OpenClaw Gateway，支持文本流式对话、会话切换与历史、模型切换、图片附件、工具调用可视化、断线重连与自动修复，并与当前 Tauri + React 控制台结构自然融合。

## Context

当前仓库是一个 Tauri 2 + React 19 + TypeScript 的 OpenClaw 桌面控制台，已有模型、频道、Agent、网关启动与修复等能力，但没有应用内聊天页。

对比结果：

- OpenClaw 官方 WebChat 走的是 Gateway WebSocket 原生协议，并将连接层、聊天控制器、工具流和视图层分离。
- `clawpanel` 也是原生聊天，但更偏桌面壳实现：大量页面逻辑集中在单文件中，设备签名、自动配对和 origin 修复放在 Tauri/Rust 侧，并额外做了本地消息缓存。

本设计采用“官方分层 + `clawpanel` 桌面增强”的混合方案：

- 学官方：协议层、控制器层、工具流层、视图层拆分。
- 学 `clawpanel`：Tauri 侧设备签名、自动配对/修复、本地历史缓存。

## Requirements

第一版必须支持：

- 应用内聊天页与导航入口
- Gateway WebSocket 握手与自动重连
- 文本流式输出
- 会话列表、会话切换、历史加载、会话重置
- 模型切换与常用 slash command 支持
- 图片附件上传与发送
- 工具调用流与结果可视化
- 连接错误态、自动修复、手动重试
- 本地缓存最近聊天记录，优先秒开

第一版不做：

- 多端共享本地缓存同步
- 语音输入 / STT
- 托管 Agent / 自动循环执行
- 富媒体编辑器、拖拽排序、复杂消息草稿同步

## Approach Options

### Option A: 直接移植 `clawpanel` 聊天页

优点：

- 功能现成，能较快覆盖完整能力。

缺点：

- 与当前 React 结构不一致，后续维护成本高。
- 单文件状态过大，不适合作为当前仓库的长期基础。

### Option B: 复用协议思路，按当前仓库重写 React 原生聊天

优点：

- 能融入现有应用结构。
- 后续扩展和维护成本最低。
- 便于做性能优化和针对桌面端的增强。

缺点：

- 首轮开发量高于直接移植。

### Option C: 继续做 Webview / dashboard 嵌入

优点：

- 短期最省事。

缺点：

- 不符合“应用原生聊天”的目标。
- 当前 Control UI 存在 `X-Frame-Options: DENY` 与 `frame-ancestors 'none'` 约束，不能作为主方案。

## Chosen Design

采用 Option B。

系统分为四层：

1. Rust / Tauri 命令层
2. Gateway 协议客户端层
3. 聊天状态与控制器层
4. React 视图层

## Architecture

### 1. Rust / Tauri 命令层

新增 `src-tauri/src/chat.rs`，职责：

- 读取 Gateway 连接信息（端口、token）
- 生成 Gateway `connect` 帧
- 生成 / 持久化设备身份
- 自动配对设备
- 修复 `gateway.controlUi.allowedOrigins`
- 暴露聊天页所需的最小命令集

这层参考 `clawpanel`，因为桌面应用无需依赖浏览器安全上下文来完成 Ed25519 签名，也更适合将配对和修复逻辑做成一键恢复。

建议新增命令：

- `chat::get_gateway_connection_info`
- `chat::create_connect_frame`
- `chat::auto_pair_device`
- `chat::check_pairing_status`

### 2. Gateway 协议客户端层

新增 `src/lib/chat/gateway-client.ts`，职责：

- 建立 WebSocket 连接
- 处理 `connect.challenge`
- 请求 Rust 生成签名后的 `connect` frame
- 统一封装 `request(method, params)`
- 管理重连、ping、握手超时、状态广播
- 向上层派发 Gateway 事件

协议能力覆盖：

- `chat.send`
- `chat.history`
- `chat.abort`
- `sessions.list`
- `sessions.delete`
- `sessions.reset`

客户端不直接处理 React UI 状态，只暴露：

- 连接状态
- 当前会话默认值
- request API
- event subscription API

### 3. 聊天状态与控制器层

新增：

- `src/lib/chat/chat-store.ts`
- `src/lib/chat/tool-stream.ts`
- `src/lib/chat/message-cache.ts`
- `src/lib/chat/types.ts`

#### chat-store

职责：

- 维护当前 sessionKey
- 维护会话列表
- 维护消息历史
- 维护流式文本缓冲区
- 维护发送队列
- 维护附件列表
- 维护错误态与连接态
- 将 UI 层事件转成 Gateway 请求

核心状态：

- `connection`
- `sessionKey`
- `sessions`
- `messages`
- `stream`
- `toolCalls`
- `queue`
- `attachments`
- `selectedModel`
- `error`

#### tool-stream

职责：

- 独立管理 `agent/tool` 流式事件
- 维护 `toolCallId -> entry` 索引
- 将流式工具事件转换成稳定的 UI 数据
- 节流同步，避免高频 tool update 触发整页刷新

该层参考官方 `app-tool-stream.ts`，避免将工具流拼接逻辑塞进聊天页面组件。

#### message-cache

职责：

- 用 IndexedDB 持久化最近聊天消息
- 以 `sessionKey + timestamp` 为索引
- 页面进入时先读本地缓存，再异步回补 `chat.history`
- 最终以 Gateway 历史为准，纠正本地乐观消息

缓存策略：

- 每个 session 仅保留最近 200 条
- 工具调用结果仅保存最终态，不保存每个中间 update
- 图片仅缓存已发送消息的必要预览数据

## UI Design

新增页面与组件：

- `src/components/ChatPage.tsx`
- `src/components/chat/ChatSidebar.tsx`
- `src/components/chat/ChatMessageList.tsx`
- `src/components/chat/ChatComposer.tsx`
- `src/components/chat/ToolCallCard.tsx`

### 页面布局

- 左侧：会话列表
- 中间：消息区
- 底部：输入框 + 附件 + 发送 / 停止按钮
- 顶部：连接状态、模型选择、刷新、新建会话

### 交互要求

- 发送时先乐观显示用户消息
- assistant 流式输出显示单一活动气泡
- 工具调用显示为独立可展开卡片
- 断开连接时显示遮罩或顶部提示，不阻塞已渲染历史
- 初次进入时如果网关不可用，提供“修复并重连”

## Data Flow

### App boot / page enter

1. 用户进入聊天页
2. 前端调用 `get_gateway_connection_info`
3. 建立 WebSocket
4. 收到 `connect.challenge`
5. 调用 `create_connect_frame`
6. Gateway 握手成功，得到默认 `mainSessionKey`
7. 先读取 IndexedDB 缓存
8. 再请求 `chat.history`
9. 刷新会话列表

### Send flow

1. 用户输入文本 / 附件
2. 本地乐观插入用户消息
3. 调用 `chat.send`
4. `chat state=delta` 更新流式 buffer
5. `agent stream=tool` 更新工具流
6. `chat state=final` 合并成最终 assistant 消息
7. 写入 IndexedDB
8. 若需要，刷新会话列表摘要

### Recovery flow

1. 连接失败或 `origin not allowed` / `pairing required`
2. 前端触发 `auto_pair_device`
3. Rust 修复 `allowedOrigins` 并写入设备身份 / paired state
4. 重连 Gateway
5. 成功后重新加载历史

## Error Handling

必须覆盖：

- Gateway 未启动
- token 缺失或无效
- pairing required
- origin not allowed
- handshake timeout
- `chat.send` 失败
- `chat.history` 加载失败
- 图片转码失败 / 超过大小限制
- 同一 run 多次 `final`

容错策略：

- 对 `delta/final` 做 runId 去重
- 工具事件按 `toolCallId` 合并
- 如果 `final` 结构无法可靠归并，则回退到重新请求 `chat.history`
- 会话切换时清理旧 session 的流式态与未完成工具流

## Performance Strategy

性能目标：

- 打开聊天页后优先在 200ms 内展示已有本地历史
- 流式更新不因 token 级频率导致明显掉帧
- 长会话下滚动和输入保持流畅

具体策略：

- 流式文本按节流频率批量更新，不逐 token 全量重渲染
- 工具流单独维护并节流同步
- 历史与当前流式态分离
- 消息列表组件只依赖归一化后的显示数据
- 大图片发送前限制大小并压缩预览开销
- 会话列表与消息区分开更新，减少无关重渲染

## Testing Strategy

### Rust

- 设备身份生成与持久化
- connect frame 生成格式
- pairing 状态判断
- `allowedOrigins` 修复逻辑

### Frontend unit

- Gateway client 握手与重连
- `chat.send/history/abort` 封装
- delta/final/tool 事件归并
- 本地缓存读写
- 会话切换时状态清理

### Integration

- 当前本机 OpenClaw Gateway 联调
- 能发送文本消息
- 能停止生成
- 能切换模型
- 能看到工具调用
- 能发送图片附件
- 能在断开后恢复连接

## File Plan

### Modify

- `src/App.tsx`
- `src/components/app-sidebar.tsx`
- `src/types.ts`
- `src-tauri/src/lib.rs`

### Create

- `src/components/ChatPage.tsx`
- `src/components/chat/ChatSidebar.tsx`
- `src/components/chat/ChatMessageList.tsx`
- `src/components/chat/ChatComposer.tsx`
- `src/components/chat/ToolCallCard.tsx`
- `src/lib/chat/gateway-client.ts`
- `src/lib/chat/chat-store.ts`
- `src/lib/chat/tool-stream.ts`
- `src/lib/chat/message-cache.ts`
- `src/lib/chat/types.ts`
- `src-tauri/src/chat.rs`

## Decisions

- 使用原生 React 聊天页，不嵌入 dashboard / iframe
- 使用 Tauri/Rust 生成设备签名和执行配对修复
- 使用 Gateway WebSocket 作为唯一实时通道
- 使用 IndexedDB 做桌面端近期历史缓存
- 以官方 OpenClaw 的分层结构为主，以 `clawpanel` 的桌面增强为辅

## Open Questions

当前没有阻塞实现的未决问题。

若后续发现不同 OpenClaw 版本在 `chat final` 或 `agent tool` 事件结构上存在差异，则优先通过事件标准化与 `chat.history` 回补兜底，而不是让 UI 直接依赖不稳定字段。
