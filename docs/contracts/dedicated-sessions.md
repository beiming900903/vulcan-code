# 专职会话（专职助手）

## 权威与同步触发点

本契约归属于 `vulcan-session-core::v1::dedicated`（配置形状、区间不变量与 Provider 事件筛选）和
`vulcan-session-core::v1::document::dedicated`（安装、记忆锁定与冻结设置守卫）。修改专职会话的持久字段、
记忆规则、请求组成或界面语义时，必须同步本文、行为测试与 WebUI/TUI 渲染。持久字段变化同时遵守
[会话元数据版本](session-metadata-migrations.md)。

## 会话种类

- 专职会话是只能在新建时选择的独立会话种类，`MetadataDocument.dedicated` 为 `Some` 即为专职会话，
  普通会话为 `None`。两者之间不可相互转换。
- 专职会话只能由草稿首次提交创建：`sessionSettings.dedicated`（`toolAccess`、`basePrompt`、
  `lockImmediately`）随首轮发送；已存在的会话收到该字段时返回 `ERR_DEDICATED_SESSION_NOT_CONVERTIBLE`，
  同时设置 Goal 或 Plan 模式返回 `ERR_DEDICATED_SESSION_MODE_UNSUPPORTED`。
- 安装只能发生在刚创建、尚无任何事件的会话上，并先于根输入写入（因此立即锁定的锁定标记位于首条用户
  消息之前）。随后首轮的会话设置（回答模式、输出风格、知识库、VMM 关闭）照常写入。
- **冻结点**：首轮已开放、全部事件仍驻留且尚未出现 `ApiStarted` 的“创建窗口”内允许写入上述设置；窗口
  关闭后，以下设置的真实修改一律返回 `DedicatedSettingFrozen`：回答模式、输出风格、选择配置包、知识库
  选择、VMM 策略、Goal、Plan 模式，以及编辑重生成与分叉。会话请求选项（缓存时长、服务档位）属于
  请求级选择，与模型和思考档位一样不冻结。
- 基础提示词由用户在创建时提供（可借助当前模型生成或优化），创建后永不修改；上限只在
  `DEDICATED_BASE_PROMPT_MAX_CHARS` 定义。行为调整通过解锁后的补充对话完成，而不是改写基础提示词。
- 基础提示词可通过 `service_draft_dedicated_prompt` 起草或优化：使用输入区所选的精确供应商、连接与模型
  （不改道），系统提示词为 `agent-prompts/dedicated-prompt-generator.md`；输入与输出都受
  `DEDICATED_BASE_PROMPT_MAX_CHARS` 约束，结果只返回给客户端审阅，不写入任何会话。思考档位与会话回合一致：
  省略时使用模型自身默认档位（`resolve_effective_public_reasoning_effort`），显式档位必须是模型声明的档位，
  否则返回 `ERR_DEDICATED_PROMPT_DRAFT_REASONING_INVALID`，绝不把空档位直接交给供应商。请求携带客户端
  `progressId` 时，服务以 `dedicatedPromptDraftProgress` 事件（主题 `dedicated.prompt_draft.progress`）推送
  节流后的思考与输出增量；进度只用于展示，最终结果以 RPC 响应为准。
- `tool_access`：`none` 不开放通用工具（仅保留创建时启用的知识库工具），也不注入提示词技能、工作区
  信息与权限模式通知；`standard` 开放通用工具但排除子代理、协作、Goal、Plan 与 VMM 记忆工具。
  两种方式都不使用子代理、上下文压缩（自动压缩跳过，手动压缩报错）与 VMM 记忆。
  知识库四工具为 `knowledge_list`、`knowledge_read`、`knowledge_search`、`knowledge_write`；两档均保留创建时启用库
  所允许的工具，全部库仅浏览时不注册搜索。库模式决定检索方式，会话不保存检索策略；写入仍实时复核授权。
  内容与生命周期边界见 [AI 管理知识库](knowledge-base.md)，修改工具过滤或选择字段时同步两份契约及测试。
- 请求组成：基础提示词作为代理基础提示词发送，不附加能力指引；`none` 使用提示词组装模式
  `dedicated_minimal`（代理基础、输出风格、工具说明），`standard` 使用 `dedicated_tooling`（另含供应商基础、
  项目规则与环境上下文）。回答模式提示照常提供。

## 记忆规则

- `locked_spans` 是有序、互不重叠的锁定区间，只有最后一个可以仍然开放（`unlocked_at_seq: null`）。
  每次锁定或解锁在同一修订中追加一条会话级 `DisplayNote`（`memory_locked` / `memory_unlocked`，
  `UiOnly`）并更新区间；区间端点就是这两条通知的 seq。
- 锁定与解锁只能在回合之间进行，开放回合期间返回 `DedicatedMemoryLockWhileTurnOpen`
  （服务错误码 `ERR_DEDICATED_MEMORY_LOCK_TURN_OPEN`）。服务入口为 `service_set_dedicated_memory_lock`，
  重复当前状态为幂等空操作；响应携带更新后的配置。客户端从事件窗口的 `session_config.dedicated` 读取配置。
- 起点位于锁定区间内、且已不是当前开放回合的回合称为“不记忆回合”。`dedicated_provider_events` 按整回合
  移除这些回合的事件（保持 API 组结构完整），但始终保留 `SystemContext` 语义，因为它们是会话级指令，
  幂等同步不会重新发出。当前开放回合始终完整保留，因此一轮之内可以调用工具和插入补充输入。
- 解锁后从解锁点重新累积记忆；锁定期间的不记忆回合不会被带回。再次锁定产生新区间，之后每轮以
  “全部已记忆内容”为基准。

## 浮动助手

- 记忆处于锁定状态的专职会话可以转换为浮动助手：它离开会话列表，以用户另取的名称作为以项目为归属、全局
  可用的浮动窗口出现。这只是列表与呈现状态，会话种类、元数据与事件历史都不变，因此不违反“种类不可转换”。
- 状态只存放在 AppState 库的 `session_assistant_index_v1`（主键外键级联于 `session_dedicated_index_v1`），
  与 `unread` 同类，不写入会话元数据，`METADATA_SCHEMA_VERSION` 不变。转换时一并补写专职标记，因此该标记表
  引入前创建的专职会话同样可以转换。名称上限只在 `SESSION_ASSISTANT_NAME_MAX_CHARS` 定义，经
  `SessionSummary.assistant_name` → `ConversationSummary.assistantName` 下发。
- 服务入口：`service_convert_conversation_to_assistant`（`conversationId`、`name`）在生命周期过渡锁内校验
  专职会话（`ERR_DEDICATED_SESSION_REQUIRED`）、记忆已锁定（`ERR_DEDICATED_ASSISTANT_REQUIRES_LOCK`）与名称
  （`ERR_DEDICATED_ASSISTANT_NAME_REQUIRED` / `_NAME_TOO_LONG`），重复转换即改名；
  `service_restore_assistant_conversation`（`conversationId`）删除该行，会话回到列表。两者都返回刷新后的工作台
  快照。
- 助手状态下解锁记忆返回 `ERR_DEDICATED_ASSISTANT_UNLOCK_REFUSED`：先还原为会话再解锁。

## 界面语义

- 不记忆回合照常显示并标注“不记忆”；锁定、解锁通知显示为时间线标记。
- 基础提示词在时间线开头以用户消息样式显示，并标注其实际作为系统提示词发送。
- 会话列表以种类标记区分专职会话。列表投影来自 AppState 库的 `session_dedicated_index_v1`（外键级联于
  `session_index_v1`），只在 `create_indexed_session_v1` 按 `metadata.dedicated` 写入，经
  `SessionSummary.dedicated` → `ConversationSummary.dedicated` 下发；列表加载不逐个读取元数据文件。
  该表引入前创建的专职会话没有标记（V0.1.x 不迁移）。

### WebUI

- **新建入口**：侧栏项目分组提供全局“新建对话”按钮；草稿输入区上方的标签行选择项目、会话种类（工作模式 /
  专职助手）与工具开放方式（无工具 / 有工具）。种类与工具开放方式只在首次提交前可改，提交挂起时全部冻结。
- **创建向导**：选择专职助手后输入区替换为三步顺序向导（`components/composer/dedicated/`）：基础设置、
  基础提示词、记忆方式。基础设置同页分区显示知识库、回答模式与输出风格，选项以带选择标记的卡片呈现（回答
  模式附效果示意，输出风格附提示词长度横条）；确认项目存在知识库（或加载失败）后才显示知识库分区。回答模式与
  输出风格不提供“跟随全局”选项，也不跟随全局设定：未改选时默认选中带“推荐”标记的标准回答与不限定风格
  （`DEDICATED_RECOMMENDED_*`，因为助手按自身提示词作答，交互组件与额外风格通常不适用），并在创建时以显式值
  提交（仅限创建向导；普通会话的继承全局语义不变）。向导状态按本地草稿身份保存，更换项目时保留进度，但会剔除
  不属于新项目的知识库选择。基础提示词步骤先选择会话模型与思考档位（复用输入区的模型/账号/思考档位控件与草稿规则：模型即切换输入区当前
  精确模型，档位保持本地、默认取模型声明的档位，创建时随首轮提交为 `reasoningEffort`），用户
  填写的文本即基础提示词；“生成提示词”打开独立弹窗，在弹窗内描述需求、起草并按修订要求优化（起始草稿为向导
  当前提示词），只有点击“写入提示词”才回填向导。弹窗只读显示该会话模型并用它起草，思考档位默认取该模型的
  默认档位并可在弹窗内改选，生成期间实时显示思考过程。向导显示期间输入区停靠层占满会话高度，较高的步骤在向导内部滚动，空状态标题
  随之淡出；最后一步选择“立即锁定”或“先对话几轮再锁定”，以基础提示词作为首轮输入提交
  `sessionSettings.dedicated`。基础提示词长度只由服务端校验，WebUI 不复制上限。
- **已创建会话**：输入区上方显示记忆锁定条，回合进行中禁用切换。创建时冻结的设置在菜单中直接隐藏（Goal、
  Plan、回答模式、输出风格、知识库、VMM、供应商工具与 AI 能力开关），`none` 另隐藏权限模式，不提供会被服务
  拒绝的操作。转录中的用户消息编辑与回合分叉按钮同样不显示（编辑重生成与分叉在创建后即冻结）。
- **快速指令**：服务目录只提供稳定 `id` 与 `command`，说明文案为共享键 `composer.slash.<id>`。
  `composerSlashCommandsForSession`（`components/composer/composer-model.ts`）按会话种类筛选：专职会话隐藏
  被冻结或不适用的指令（`/compact`、`/plan`、`/goal`、`/style`、`/answer`、`/knowledge`、
  `/memory`），`/skill` 与 `/skills` 仅在 `standard` 工具开放方式下显示；未知 `id` 一律不显示。
- **侧栏标记**：专职会话行在标题前显示 `Bot` 图标，提示文字为 `composer.draft.kindDedicated`。
- **浮动助手**（`components/assistant/`）：记忆锁定条在锁定后提供“转换为助手”并弹出命名对话框；转换的会话
  若正被选中，主视图移到同项目的新草稿（不自动选择用户未选的其他会话）。侧栏列表排除带 `assistantName` 的会话，但它们仍留在
  `workbench.conversations` 中供停靠控制器使用。
  - 转换成功后显示确认提示并打开浮窗。浮窗顶部常驻记忆锁定状态和专职用途说明。最后一轮收到同一回合的业务
    结束事件后，按回合起点与生效锁定区间判定是否显示“本轮内容不会带入下一轮”。
  - 助手永远不会被自动选入主视图：工作区索引的默认选择（`loading.rs`）跳过带 `assistant_name` 的会话，
    WebUI 的归档回退与后端选择校验（`state/workbench-state.ts` `fallbackMainViewConversationId`）同样排除助手；
    仅剩助手时主视图进入新建草稿（`empty-workbench-route.ts`）。
  - 顶部栏在终端按钮旁显示助手按钮（存在助手时）：悬停列出助手，点击某个助手显示或隐藏；窗口以相同的
    精灵动画最小化收入该按钮。
  - 每个窗口容纳一个或多个助手；多于一个时左侧首字栏切换。新显示的助手加入最近使用的窗口；标题栏提供
    分离到独立窗口、合并其他窗口、还原为会话、从窗口隐藏与关闭窗口。窗口布局只存在于界面内存，几何位置
    沿用浮动图层的本地存储。
  - 运行状态取自工作台行的实时 `runPhase`（与主输入区相同的 `conversationIsRunning`），不读事件窗口的
    `open_turn_id`，后者只在加载时更新。
  - 窗口只显示最后一轮（`assistant-last-turn.ts`）：思考在流式输出时内联展开、结束后收起；每次请求下方以
    一行文字写出用量，本轮合计固定在输入框上方，均不使用悬停提示。每段已完成的回答在用量行旁提供复制按钮，
    复制其 Markdown 原文。完整内容仍在会话中实时同步。
  - 输入框接受粘贴多模态素材，复用主输入区的 `useComposerAttachments` 暂存与附件条；可以只发送素材不带文字。
    门控来自 `service_model_attachment_capabilities`（`providerId`、`connectionId`、`modelId`，取自
    `session_config.current_request`），只描述该路线本身，`bridged` 恒为 false。`tool_access` 为 `none` 时只接受
    `native` 类别，被拒的素材以 `assistantDock.attachmentNativeOnly` 提示；`standard` 时与普通会话接受相同类别。
    最后一轮的提示词附件以只读附件条显示。
  - 窗口样式 `assistant-floating-layer.css` 随懒加载分块载入，不进入首屏样式预算。
- **实时锁定状态**：事件窗口的 `session_config` 只在加载时更新，因此生效区间由加载时的
  `session_config.dedicated.locked_spans` 加上其最后端点之后到达的 `memory_locked` / `memory_unlocked` 通知
  派生（`service/dedicated-memory.ts`）；判定规则与上文“不记忆回合”一致，当前开放回合从不标注。
- 错误按稳定错误码本地化，文案位于共享目录 `crates/vulcan-i18n/catalogs/webui/` 的 `composer.draft.*`、
  `composer.dedicated.*` 与 `conversation.dedicated*` 键。

### TUI

- 已保存专职会话的知识库页按工作台 `dedicated` 标记只读展示当前有效选择，继承及库开关不携带写入动作；
  TUI 不参与专职会话的创建窗口设置。
- TUI 不提供创建或锁定入口；锁定、解锁通知以共享键 `conversation.dedicatedNotice.locked` / `.unlocked`
  显示在记录区（`backend/service/map.rs`），不显示原始 `reason_code`。
- 侧栏会话行以 `ConversationSummary.dedicated` 在标题前显示主色 `◆` 种类标记（`ui/sidebar.rs`）。
- TUI 没有浮动助手界面；带 `assistantName` 的会话不进入侧栏行（`backend/service/worker.rs`
  `conversation_rows`），转换与还原只在 WebUI 进行。
