# 阿里百炼 Provider 预设与内置模型列表设计

## 背景

当前模型管理页支持通过 Provider 预设或手动填写 `baseUrl`、`apiKey`、`providerName` 新增模型提供方。

现有远端拉取逻辑在后端统一走 `fetch_remote_models`，其中：

- `openai-completions` 默认请求 `GET {baseUrl}/models`
- `anthropic-messages` 请求 `GET {baseUrl}/models` 并附加 Anthropic 头部

阿里百炼在当前配置下无法按现有 OpenAI 兼容模型列表接口稳定拉取模型，因此需要单独兼容。

## 目标

- 在模型管理页新增“阿里百炼”预设
- 选择阿里百炼预设后，用户仍然沿用现有“获取模型列表 -> 勾选模型 -> 同步”流程
- 当 Provider 为阿里百炼时，“获取模型列表”直接返回内置模型列表，不依赖远端 `/models`
- 同步到配置文件时，为百炼模型写入预定义元数据，并为指定模型写入 `compat.thinkingFormat = "qwen"`

## 非目标

- 不重新引入“兼容协议”手动选择 UI
- 不新增单独的“模板导入”页面或额外交互模式
- 不修改普通 OpenAI / Anthropic Provider 的拉取逻辑

## 方案概览

### 前端

在模型管理页预设列表中新增 `bailian`：

- `title`: `阿里百炼`
- `badge`: `官方`
- `baseUrl`: `https://coding.dashscope.aliyuncs.com/v1`
- `defaultName`: `bailian`
- `apiAdapter`: `openai-completions`

用户点击该预设后，页面行为与其它预设保持一致：

- 自动填入 URL 和默认名称
- 点击“获取模型列表”
- 进入模型勾选页
- 点击“同步模型”

前端不新增专属模式字段，不额外透传“百炼标记”。是否属于百炼由后端根据 `baseUrl` 识别。

### 后端

#### 1. 百炼识别

在 `src-tauri/src/models.rs` 中新增百炼地址识别逻辑，命中以下主机时视为百炼 Provider：

- `coding.dashscope.aliyuncs.com`

识别方式只依赖 `baseUrl`，以便新增、刷新和重同步都走同一条后端逻辑。

#### 2. 内置模型列表

在后端维护一份百炼内置模型定义表，至少包含以下模型：

- `qwen3.5-plus`
- `qwen3-max-2026-01-23`
- `qwen3-coder-next`
- `qwen3-coder-plus`
- `MiniMax-M2.5`
- `glm-5`
- `glm-4.7`
- `kimi-k2.5`

每个定义包含：

- `id`
- `name`
- `reasoning`
- `input`
- `contextWindow`
- `maxTokens`
- 可选 `compat.thinkingFormat`

#### 3. 获取模型列表

修改 `fetch_remote_models`：

- 若识别为百炼，直接返回内置模型 ID 列表
- 不请求远端 `/models`
- 返回值仍保持当前格式：JSON 字符串数组

这样前端无需改动获取后的展示和勾选逻辑。

#### 4. 模型配置生成

扩展当前模型 JSON 构建逻辑：

- 普通 Provider 继续沿用现有自动探测逻辑
- 百炼 Provider 命中内置定义时，优先使用内置元数据生成模型配置
- 对以下模型写入：
  - `compat.thinkingFormat = "qwen"`
  - `qwen3.5-plus`
  - `qwen3-max-2026-01-23`
  - `glm-5`
  - `glm-4.7`
  - `kimi-k2.5`

同步后生成的配置应尽量贴近官方示例，包括：

- `cost` 默认保留为全 0
- `contextWindow`
- `maxTokens`
- `input`
- `compat`

#### 5. 刷新 Provider

刷新复用 `fetch_remote_models`：

- 百炼 Provider 刷新时仍显示内置模型列表
- 用户可以继续通过勾选决定保留哪些模型
- `reconcile_provider_models` 为新增百炼模型补齐内置元数据

## 数据流

1. 用户在模型管理页点击“阿里百炼”预设
2. 前端填入 `baseUrl=https://coding.dashscope.aliyuncs.com/v1`
3. 用户点击“获取模型列表”
4. 后端识别为百炼，直接返回内置模型 ID 列表
5. 用户勾选模型并点击“同步模型”
6. 后端按百炼内置定义写入 Provider 配置和 `agents.defaults.models`
7. 若主模型缺失，继续沿用现有主模型修复逻辑

## 错误处理

- 百炼 Provider 获取模型时，不因为远端 `/models` 不可用而失败
- 若用户手动把百炼地址改成其它域名，则回退到普通 `openai-completions` 流程
- 若同步时选中了不在内置定义中的模型：
  - 优先回退到现有自动探测逻辑生成最小模型配置
  - 不阻断整个同步流程

## 测试

### 前端测试

- 新增阿里百炼预设会正确填入默认 URL、名称和协议
- 点击百炼预设后，“获取模型列表”仍沿用原有页面流转

### Rust 单元测试

- 百炼地址命中时，`fetch_remote_models` 返回内置模型列表
- 普通 OpenAI 地址不触发百炼特判
- 百炼模型配置生成时写入正确的 `input/contextWindow/maxTokens`
- 指定模型会包含 `compat.thinkingFormat = "qwen"`
- `reconcile_provider_models` 新增百炼模型时也能补齐同样的元数据

## 预期结果

完成后，用户在模型管理页只需要：

1. 选择“阿里百炼”预设
2. 填写 Key
3. 获取模型列表
4. 勾选并同步

即可得到一份接近官方示例的百炼 Provider 配置，无需手工编辑 JSON。
