# LuaSkills 技能包配置接入指南

## 1. 适用范围

本文说明 Vulcan Code 的 LuaSkills 技能包配置入口，面向技能包作者、宿主维护者和桌面端使用者。当前独立集成采用正式 0.9.0 注册表依赖，其版本和来源以根清单与实际锁为准。

技能配置声明以当前上游源码及已有生成技能配置契约为准，不执行旧配置探测、迁移、别名兼容、旧文件提示或自动清理。本文后续零点五点九版本引用保留历史契约与验证背景；不能用于证明零点六候选的构建来源。正式插件执行契约另见[插件执行引擎归属](plugin-execution-engine.md)。

### 1.1 当前正式依赖与本地验证边界

根 `Cargo.toml` 精确声明 `luaskills = "=0.9.0"`，没有 LuaSkills 本地 path patch。实际锁的来源为
`registry+https://github.com/rust-lang/crates.io-index`，校验和为
`3993b8ff5960b80f3bef2f7c923b2b1c0c1963b4b9499331a0d5450c7a5c9ce0`。该版本在 Core 五平台 GitHub
发布（标签 `v0.9.0`）全部门禁通过后才发布到 crates.io。`workspace-hack/Cargo.toml` 中的 `luaskills`
条目由 `cargo hakari generate` 维护，升级后用 `cargo hakari generate --diff` 确认无差异。

0.7.3 起 LuaSkills 不再拥有数据库存储：SQLite 只经宿主注册的回调承载，LanceDB 能力与
`vldb-controller-client` 依赖均已移除。宿主在创建引擎前通过
`host_database_providers::install` 注册 SQLite 回调；`build_host_options` 设置
`host_mode = LuaRuntimeHostMode::Desktop`，使 Windows 上运行时拥有的控制台子进程不弹出窗口。
运行时根属于只读发行树：`build_host_options` 把 `temp_dir` 与 `download_cache_root` 设为用户缓存下的
`cache/luaskills/temp` 与 `cache/luaskills/downloads`，并设置 `preserve_host_writable_dirs = true`
（LuaSkills 0.9 起），因此 LuaSkills 不再在运行时根下创建 `temp`；`temp_dir` 由宿主创建，下载缓存由引擎创建。
Core 在创建线程和 Lua VM 前移除继承的 `LUA_PATH`、`LUA_CPATH`、`LUA_INIT` 及其带版本后缀的形式。
Windows 同时清理 LuaJIT 读取的 C 运行时环境。该隔离只作用于 Core 进程及其继承环境的子进程。
LuaJIT 通过 Windows 的 ANSI 接口获取可执行文件目录并展开默认模块路径，Core 的可执行文件清单为进程启用
UTF-8 代码页（Windows 10 1903 及以上），保证中文等非 ASCII 安装路径能按 UTF-8 进入 LuaSkills。
`crates/vulcan-service-runtime/managed-runtimes.json` 的 `luaskills_version` 必须与根清单版本一致，
否则构建脚本拒绝构建。

开发 Core 期间可以临时在根清单 `[patch.crates-io]` 指向本地 Core 检出做联调，但提交前必须移除并重新从
注册表解析；path 来源没有注册表校验和，不能作为正式依赖的证据。

### 1.2 已发布零点五点九依赖的历史记录

以下正式依赖记录描述原零点五点九接入及主工作树现状，保留原始版本与校验和；它不是本独立零点六候选的构建声明。

根目录 `Cargo.toml` 精确锁定 `luaskills = "=0.5.9"`；`Cargo.lock` 必须从 crates.io 注册表解析该版本及其校验和。应用构建只使用普通的 `make build`、`make br` 和对应 release 入口，不再通过 Make 目标或脚本覆盖本地 LuaSkills 检出。

本次锁文件中的依赖身份为：

```text
name = "luaskills"
version = "0.5.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3f04f7301f2fd516b14f5a6a90849294b51dbd09156d0fccba5cd74ad31633b9"
```

`MAX_RUNTIME_SESSION_LEASES_PER_MANAGER` 等所需接口由正式 0.5.9 公开。宿主的会话入场容量直接引用上游常量，不再维护兼容镜像。本指南后文保留的本地上游路径与测试取证仅解释历史行为和待改进边界，不是构建来源。

本文中描述上游契约的章节（见 §3）同样以该 crate 的对应版本为准，本仓库不复制其字段校验。

### 1.3 当前候选的上游契约回读

此前回读候选提交 `df34cbd87bd223a0a01fba22cd7223767e466291`，并以正式零点五点九对照提交
`71d94be113b309004c4d4aa2b18ce43b1e867aaf` 核对技能配置、监听、System 目录、安装来源、
Managed IO 与受管环境缓存。下文对应旧技能入口的规则保持原行为；新增正式插件执行归属另见插件契约。
源码存在的测试仅说明已有覆盖，不表示本轮候选多平台、正式注册表或真实宿主现场验收已经完成。
上游版本、这些入口或宿主根依赖变化时，须重新回读源码并同步本文；历史验证不替代该步骤。

当前正式零点六点一来源对应 Core 提交 `7320b8a21a4833afd0d94a260108d0bf755304cd`。本次从实际 registry 包回读 `src/skill/manifest.rs` 与 `src/skill/config.rs`，两份字节均与该冻结 Core 来源一致；第 3 节的技能配置声明仍以上游类型及校验为权威。本轮依赖身份确认不替代旧行为、正式构建、多平台或真实宿主现场的重新验收。

## 2. 宿主持久化位置

Vulcan Code 为宿主中的 LuaSkills 引擎传入同一个固定配置根：

```text
<application_data_root>/luaskills-config
```

生产环境默认展开为：

```text
<user-vulcan-root>/code/luaskills-config/
├── skills/
│   └── config.json
└── system-skills/
    └── config.json
```

其中 `<user-vulcan-root>` 会随构建配置变化：release 为 `~/.vulcan`，debug 为 `~/.vulcan-debug`。

普通 USER 技能使用 `skills/config.json`，ROOT 系统技能使用 `system-skills/config.json`。配置根与技能安装目录、构建产物和运行时资源目录相互独立。

插件执行引擎独立于普通技能管理引擎；共享配置路径不表示共享引擎或执行锁。当前接入及迁移边界见[插件执行引擎归属](plugin-execution-engine.md)。

## 3. 技能包作者声明配置

> **契约归属。** 本节字段契约由上游 `luaskills` crate 拥有，候选及正式来源边界见第 1 节。
> 权威为上游 `src/skill/manifest.rs` 的 `SkillMeta` 以及 `src/skill/config.rs` 的声明类型和校验。
> 当前正式注册表包的这两份源码已与冻结 Core 来源核对一致；本仓库不复制上游字段校验。
> 升级版本时必须回读对应源码并同步本节，不能以本文代替实际 Schema 和类型检查。

配置声明必须位于 `skill.yaml` 顶层 `config`，不能挂在单个 entry 下。以下示例展示常用字段：

```yaml
id: example-configured-skill
name: Example Configured Skill
version: 1.0.0
enable: true

config:
  - key: api_token
    type: string
    required: true
    sensitive: true
    title: API Token
    description: Token used to call the remote service.
    format: password

  - key: retry_count
    type: integer
    required: false
    default: 3
    title: Retry Count
    description: Maximum request retry count.
    constraints:
      minimum: 0
      maximum: 10

  - key: output_mode
    type: enum
    required: true
    default: compact
    title: Output Mode
    description: Select the output detail level.
    options:
      - value: compact
        label: Compact
        description: Return concise output.
      - value: detailed
        label: Detailed
        description: Return detailed output.

  - key: debug_enabled
    type: boolean
    required: false
    default: false
    title: Debug
    description: Enable package debug behavior.
    advanced: true
    restart_required: true
```

正式类型只有 `integer`、`string`、`float`、`enum` 和 `boolean`。技能包应直接遵守第 1 节实际上游来源的字段、类型、约束和安全整数规则，不应依赖宿主修正错误声明。技能清单开关为 `enable`，不是插件清单的启用字段。

## 4. 技能包内读取配置

Lua 代码只能访问当前技能包自己的配置命名空间。当前上游在技能执行上下文中提供以下正式接口；该旧技能接口不自动授予所有正式插件公共配置访问权：

```lua
local token = vulcan.config.get("api_token")
local has_mode = vulcan.config.has("output_mode")
local entries = vulcan.config.list()
local descriptor = vulcan.config.describe()
local status = vulcan.config.status()
```

包内如需修改自身配置，可使用 `vulcan.config.set` 和 `vulcan.config.delete`。对跨字段业务约束，应在技能包自己的 `config_validator` 中实现，使 WebUI、宿主校验和实际调用共享同一结果。

## 5. WebUI 管理行为

LuaSkills 技能列表仅对声明了配置的技能显示配置入口，并显示“配置完整”或“配置不完整”状态。

配置弹窗按声明动态渲染：

- `boolean` 使用开关；
- `enum` 使用声明选项；
- `integer` 和 `float` 使用数值输入，并应用上下界；
- `string` 使用单行、多行、密码或路径提示对应的输入形式；
- 按 `group`、`order` 排序；
- `advanced: true` 放入折叠的高级配置区域；
- `deprecated`、`restart_required` 和校验错误均明确展示。

敏感项只显示“已配置”状态。服务端不会把敏感有效值返回给 WebUI；用户新输入的敏感值只存在于当前弹窗草稿，提交成功后立即清空。

所有写入和删除都携带打开弹窗时观察到的 `revision`。如果发生 `CONFIG_REVISION_CONFLICT`，界面会重新读取最新修订并保留未提交草稿，由用户确认后再次保存，不会自动覆盖其他运行时的修改。

## 6. AI 可见能力

宿主提供只读原生工具 `runtime-config`，只接受：

- `describe`：查看生效技能包的配置结构；
- `validate`：查看指定技能包的完整性与合法性。

宿主固定使用 `mode=effective` 和 `include_values=false`。AI 不能通过该工具执行 `get`、`list`、`set`、`delete`、`refresh`，也不能读取敏感或非敏感的当前配置值。

### 6.1 技能包帮助与调用说明

契约归属：帮助树形状以上游 `list_skill_help` / `render_skill_help_detail` 为准；宿主实现位于
`luaskills_runtime.rs`（`render_luaskills_help_trees`）、`vulcan-luaskills-prompt` 与
`tooling/bridge/luaskills_dispatch.rs`。上游帮助接口变化时回读源码同步本节。

- 入口到帮助主题的映射只取上游帮助节点的 `related_entries`，不得用入口名猜测主题；没有专属主题的
  入口由 `main` 覆盖。帮助渲染失败作为基础架构警告暴露，不得静默吞掉。
- 只要包内存在显式或隐式启用入口，就向开发者提示词注入该包的锁定片段：`main` 正文（超过
  `LUASKILL_MAIN_HELP_INLINE_LIMIT_BYTES` 时只注入说明并指向 `luaskill_help`）、宿主生成的调用说明
  （canonical 名到直接函数工具名的映射，以及经 `call_luaskill` 调用的隐式入口）和主题索引。调用说明
  依据最终可见工具集生成。
- 模型可见帮助工具只有 `luaskill_help`，不保留 `luaskill` 别名：`{skill}` 返回 `main` 与主题索引，
  `{skill, topic}` 返回单个主题，`{name}` 返回入口的主题；隐式入口另附隐藏宿主管理字段后的参数 Schema。
- 技能包作者编写的 `main` 与主题必须与调用方式无关：使用 canonical 入口名，不写宿主工具名、调度器
  或注册方式，这些由宿主调用说明负责。

## 7. 配置缺失处理

技能包声明必填项但没有显式值或默认值时：

1. LuaSkills 把对应项标记为 `missing`；
2. 技能包配置状态为不完整；
3. WebUI 显示缺失项与校验消息；
4. 用户在配置弹窗补齐后，宿主以 CAS 批量提交；
5. 提交成功后重新校验并刷新技能目录状态。

宿主不为缺失值生成候选值，不从环境变量、旧文件或其他字段猜测配置。

技能包需要引导 AI 处理缺失配置时，应使用不包含值的英文提示，例如：

```text
This skill package configuration is incomplete. Call runtime-config with
action=describe and skill_id=example-configured-skill, explain the missing
declarations to the user, and ask the user to configure them in the LuaSkills
WebUI. Do not request or relay sensitive values.
```

## 8. 安全边界

LuaSkills VM 池的宿主默认值由 `vulcan-luaskills-config::LuaSkillsRuntimeSettings::default()`
定义：主 VM 池最小 4、最大 8，多余空闲 VM 在 60 秒后可回收；隔离 runLua 池最小 1、最大 4，
多余空闲 VM 在 30 秒后可回收。缺少 `luaskills-runtime-setting.json` 时使用这些内存默认值，
不会自动创建用户配置文件。该文件位于用户数据根的 `config/` 目录（`~/.vulcan/code/config/`），发行物不附带模板，
安装目录在运行时从不写入；默认值变更时同步本段说明。已有配置仍按用户保存值加载；已有文件缺少必填池字段
仍显式报错。

服务端先读取不含值的声明，只为 `sensitive=false` 且当前合法的配置项读取和规范化有效值。写入结果同样会删除敏感项的规范值，再跨越 RPC 边界。

`sensitive=true` 是宿主披露策略，不代表 LuaSkills 配置文件提供静态加密。配置仍位于当前用户的数据目录中，部署方必须依靠操作系统账户、文件权限和磁盘安全策略保护该目录。秘密不得写入默认值、示例、日志、错误信息或技能提示词。

`managedRuntime.invokeDefaultTimeoutMs` 的系统默认值为 `60000`。当技能通过受管运行时调用
`node.invoke` 等入口且没有显式传入 `timeout_ms` 时，调用最多等待 60 秒；技能显式声明的正数
`timeout_ms` 仍然优先。旧版配置中的 `null` 会在读取时规范化为 `60000`，不会继续保留无限等待。
该配置只约束受管 Node/Python 调用，不会强制中断普通 Lua handler、同步 FFI 或
`vulcan.process.exec`；这些边界必须由对应技能或 LuaSkills 运行时自身提供明确上限。

LuaSkills 宿主会在调用等待 5 秒、30 秒以及之后每 60 秒输出一次阶段诊断。`engine_read_lock_wait`
表示仍在等待宿主引擎读锁，`engine_write_lock_wait` 表示仍在等待宿主引擎写锁，`skill_body_or_host_call`
表示已经进入 Lua 技能或其宿主调用；诊断中的 `blocking_runtime_operation` 表示 RunLua 生命周期
等通用阻塞操作已经开始。诊断只包含规范工具名、进程内调用关联 ID、阶段与耗时，不包含完整参数。

普通技能管理引擎使用读写锁：普通技能调用、一次性 runLua 和公开持久 runLua 会话取得读锁。
普通技能与一次性 runLua 分别使用主池和 runLua 池；池的 `maxSize` 限制 VM 租约数，
不单独决定实际执行并发。文件型 runLua 及带目录的公开会话仍有进程工作目录锁约束；
上游独立 System 接口使用租约逻辑目录，不修改进程目录、不取得该锁。

当前正式插件执行使用独立的不可变插件引擎及管理器唯一 `ManagedFormalRuntime`，
不进入上述普通技能管理锁。工具、Hook、Action、生命周期及启用探测由该父级统一管理，
公共或专用用途容量与 `SingleCall`、复用实例、固定会话策略分别声明；不能将旧 System 租约的
每租约专用 VM 描述套用于当前全部插件调用。权威入口为 `luaskills_bridge.rs::bootstrap_luaskills`、
`service_bootstrap.rs::prepare_vulcan_plugin_runtime` 及 `runtime/bridge/embedded.rs::new`，
资源与权限边界见[插件执行引擎归属](plugin-execution-engine.md)。独立引擎不表示整个宿主进程只有一份资源预算。

普通技能管理引擎写锁仍会排空该引擎的全部读取。技能安装、更新、
卸载、启停、重载与技能包配置写入取写锁，并在排空在途读锁之后才变更引擎状态。单次锁等待超过
1 秒时另输出 `engine-read-lock-slow` 或 `engine-write-lock-slow`；写锁慢诊断只包含管理动作标签
（如 `refresh`、`install`），不包含技能标识与来源定位值。`runtime-config` 虽只暴露只读的
`describe` 与 `validate` 动作，但其上游入口声明在 `&mut LuaEngine` 上，因此仍取写锁。

## 9. 外部修改、显式刷新与事件

当前上游 `config_service.rs` 的文件监听器观察 `skills` 与 `system-skills` 两个固定存储，监听及显式刷新语义与第 1.3 节的旧对照保持一致。WebUI 打开配置弹窗时轮询有序配置事件：

- 发现当前技能包发生本地写入或外部重载后，重新加载描述与校验状态；
- 保留用户尚未提交的草稿；
- 展示外部变更提示；
- 重载失败时只展示不包含秘密值的结构化错误。

服务内部还提供严格的 `service_refresh_luaskill_config` 宿主管理 RPC。请求只允许可选的 `storeScope`，其值由 LuaSkills 正式契约校验；省略时刷新两个已配置存储。该入口不暴露给 AI，也不作为 WebUI 的任意文件读取通道。

事件游标过期时，服务先按上游稳定错误恢复到最老保留事件之前的边界，重新构建当前 LuaSkills 配置摘要，再继续处理保留事件。不会把旧事件内容猜测成当前状态。

## 10. 生命周期

- 技能更新后，LuaSkills 重新加载声明并按新声明校验既有值；
- `update_luaskill` 直接投影上游更新事务已提交的运行时目录，不再追加第二次 `reload_from_roots`。
  无版本变化时同样直接读取当前目录；显式刷新仍保留其重载语义。
- 同步 FFI 调用返回前必须结束并等待其原生工作线程退出。宿主写锁排空在途调用不等于
  排空 DLL 内的全局后台线程；VM 销毁后 `ffi.load` 可能释放动态库，残留线程不得继续执行库代码。
  CodeKit 0.2.3 的 Tokei 统计使用每次调用独立的 Rayon 作用域线程池，退出前等待线程及其线程局部
  析构完成；不得改回进程级全局线程池或仅丢弃线程池句柄而不等待退出。
  该修复由独立 CodeKit 技能发布提供，重编译宿主不会自动替换已安装的旧版 FFI 依赖。
- 启用、停用和普通重载不修改配置文件；
- 卸载技能不会删除对应配置；
- 不再声明的持久化键显示为 orphaned，只有用户执行明确删除操作时才清理；
- ROOT 与 USER 同名技能分别保留在 `system-skills` 与 `skills` 存储中；
- 配置不完整只影响状态与技能自身行为，不会让宿主自动隐藏或注销技能工具。

## 11. 排障顺序

遇到配置问题时按以下顺序检查：

1. 根 `Cargo.toml`、实际 `Cargo.lock` 与元数据的版本及来源是否一致；正式构建要求真实注册表来源和校验和，本地候选边界见第 1.1 节；
2. `skill.yaml` 的 `config` 是否位于顶层且符合正式 schema；
3. 生效技能来自 ROOT 还是 USER，以确认对应存储作用域；
4. `runtime-config validate` 或 WebUI 校验结果中的稳定错误码；
5. 是否发生 revision 冲突；
6. 配置文件外部修改是否产生 reload error 事件。

不要通过增加旧字段回退、候选路径或多格式解析来绕过声明错误。

## 12. 执行目录与租约语义

### 12.1 权威归属与同步触发

本节描述上游独立 System 接口与普通 runLua 的目录语义，权威为 `src/runtime/engine/logical_cwd.rs`、`lease.rs`、`runlua.rs`、`managed_io.rs`、`process_session.rs` 及 `contracts/system-lease-execution.md`。第 1.3 节候选回读确认旧目录规则保持；当前正式插件父级的执行归属另见插件契约。升级时必须重新核对这些入口，本文不定义另一套执行协议。

### 12.2 System 逻辑工作目录

- 宿主仍传已授权的绝对目录；省略目录仍表示包根，不能用删除字段替代路径授权。
- System 租约在 VM 内保存逻辑目录。受管文件接口、标准 IO 兼容、`loadfile/dofile`、Lua 模块搜索、动态库路径、命令解析与子进程工作目录按该目录解析。
- System 执行不修改进程目录，也不取得普通文件型 runLua 的全局目录锁；普通 runLua 保留原行为。
- System 拒绝直接 `os.execute`、`os.exit`、`os.setlocale`、`os.tmpname`，原始 FFI 不向插件开放。子进程使用已解析的绝对程序与明确子进程目录。
- 同一 Lua VM 仍然独占；此更改不提供同一 VM 并发，也不把可信原生模块变成安全沙箱。
- Windows 根相对与盘符相对路径拒绝；中文、空格路径经宿主文件接口读取，原生 C 函数自身的文件名编码限制仍属于对应函数契约。

### 12.3 原生依赖与插件桥接审计

`plugin-bridge/vulcan-plugin-bridge/runtime/bridge.lua` 由宿主以绝对路径读取并通过源码注入；包根、授权目录与进程规格均来自宿主校验。

Context7 的 `lcurl.safe` 只接收 URL、请求头、超时、TLS 校验选项与内存回调，未传入相对证书或文件路径；`openssl.cipher` 操作内存 AES-GCM，`openssl.rand` 返回随机字节。当前声明列表无修改进程目录的模块。后续新增原生模块或文件参数时必须重新审计，不能假设 Lua 包装能够截获 C 内部文件访问。

### 12.4 验证与限制

上游明确阻塞的宿主回调尚未放行时，另一 System 租约已完成；同时持有普通 runLua 目录锁并真实改变进程目录时，System 文件、模块与命令仍使用自己的目录。恢复旧全局锁后这两项回归均失败。

历史零点五点九记录中的上游提交为 `70bb42d`，Windows、Ubuntu、macOS 原生 CI：<https://github.com/LuaSkills/luaskills/actions/runs/35960551210>。该记录保留原验证归属，不是当前零点六候选结果。同租约串行和宿主取消归属由 VulcanCode 另行测试，不能以这份上游证据替代真实页面验收。

## 13. 安装来源策略与可达路径

本节记录上游来源策略与 Vulcan 消费入口的对应关系，以及由此产生的可达性结论。

### 13.1 上游支持的四种来源类型

`SkillInstallSourceType` 共四个变体，但**并非都可用**（`skill/manager.rs:794-810`、`:858-874`）：

| 变体 | 安装 | 更新 | 说明 |
| --- | --- | --- | --- |
| `Github` | 可用 | 可用 | 正常可用 |
| `OfficialHub` | 可用 | 可用 | 正常可用 |
| `Url` | **无条件 Err** | **无条件 Err** | `public URL skill install is disabled by source policy` |
| `PrivateUrlManifest` | 可用 | 可用 | 见 13.3 的前置条件 |

`Url` 的 `Err` 分支**不带任何条件判断**，因此该来源类型在上游已经死亡。

### 13.2 Vulcan 侧曾存在的三处死来源入口（均已修复）

| 入口 | 位置 | 修复后行为 |
| --- | --- | --- |
| 安装/更新 RPC | `parse_skill_install_source_type` | 拒绝 `url`，消息点名 `github` 与 `official_hub` |
| WebUI 安装对话框 | `lua-skills-panel.tsx` 来源类型下拉 | 移除 `URL` 选项，只提供 LuaSkills Hub（默认）与 GitHub |
| ROOT 引导技能配置 | `RootSkillBootstrapSourceType::into_runtime_source_type` | 返回 `Result`，拒绝并带上技能标识 |

第三处刻意**保留该变体可反序列化**：直接删除变体会让既有配置在解析期变成一条不可读的 serde `unknown variant` 错误，修复原因就无从报告。

`official_hub` 已由宿主实现（见 13.6）。`private_url_manifest` 仍是**上游支持但宿主未实现**的来源类型，必须保持「未知类型」错误，**不得静默映射到 `github` 或 `official_hub`**。

### 13.3 `PrivateUrlManifest` 的三项前置条件

`skill/manager.rs:1262-1298` 要求：

1. `plane == SkillOperationPlane::System` —— **仅限宿主系统权威面**；
2. `config.enable_private_url_skill_install == true`；
3. **manifest URL 与 manifest 内声明的 archive URL** 都必须通过 `private_skill_source_allowlist` 校验。

Vulcan 的**配置侧已齐备**：`enable_private_url_skill_install` 与 `private_skill_source_allowlist` 都已从运行时设置注入 `host_options`（`luaskills_bridge.rs`、`luaskills_runtime.rs`）。

### 13.4 本地 HTTP 验收与生产来源边界

本节验收针对 `github` 来源；生产入口另接受 13.6 的 `official_hub`，并继续拒绝用户 `url` 来源。本地验收通过**测试专用的 GitHub API 与站点基址**把同一生产 `install_luaskill` / `update_luaskill` → Service Runtime → `LuaEngine` 路径指向回环 HTTP 服务。该做法不新增生产来源类型，也不放宽 `PrivateUrlManifest` 的 System 权威面约束。

`production_install_and_update_use_local_github_routes_safely` 已从最终磁盘和请求计数验证：

- 冷安装、升级与新版本目录发布；
- 已验证缓存的热重装，以及关闭网络后的零请求缓存命中；
- 校验和不匹配、截断响应、非法 ZIP、校验和正确但 Lua 内容无效时，旧版本和已验证缓存保持可用；
- 上游真实文件系统测试 `dependency_install_rejects_preexisting_target_symlink` 继续覆盖预存目标符号链接拒绝；ZIP crate 不保留可构造的符号链接文件类型位，因此不使用无效的伪 ZIP 符号链接夹具冒充验收。

上游下载客户端现在同时设置 10 秒建连超时和 5 分钟整次请求超时；`blocking_http_client_bounds_stalled_requests` 复用生产客户端构建器，以缩短的确定性测试时限证明服务接受连接后不返回响应也会有界失败。

### 13.5 可达验收结论

本地回环 HTTP 的安装、更新、缓存、离线与失败回滚验收针对当时的本地源码 HEAD 完成。该历史结论不表示生产 WebUI/RPC 已支持任意 URL；当前候选与提交锁的实际来源边界见第 1 节。

### 13.6 `official_hub` 来源与 LuaSkills Hub 搜索

- **来源值**：`official_hub` 的 `source` 是 Hub 扁平技能标识（如 `weather`），由 LuaSkills 引擎经 `{Hub}/api/v1/skills/{id}/resolve?version=latest` 解析下载地址；宿主不自行下载或校验压缩包。
- **Hub 基础地址只有一处定义**：`vulcan-luaskills-config` 的 `DEFAULT_OFFICIAL_SKILL_HUB_BASE_URL`（`https://hub.luaskills.com`）。`LuaSkillsNetworkSettings::effective_official_skill_hub_base_url` 在用户未填写 `official_skill_hub_base_url` 时只在内存中返回该默认值，**不回写**用户配置；引擎宿主选项与搜索客户端都使用这一结果。中国大陆用户可在“Hub 与下载源”页选择中国站 `https://hub.luaskills.cn`；宿主不按地区静默切换，账号区域提示见 §13.7，也不随 Hub 改写 GitHub 镜像地址。
- **只读元数据管理器**（`build_skill_manager_for_root`）刻意不设置 Hub 地址，它不执行安装。
- **搜索 RPC**：`service_search_luaskills_hub`（参数 `query`、`cursor`、`locale`，均可选）→ `luaskills_hub::search_official_hub`，访问运行中引擎构建时所用的同一 Hub 的 `GET /api/v1/skills/search`。搜索不获取 LuaSkills 管理锁；查询长度、游标格式、响应大小、超时与重定向（禁止）都在 `luaskills_hub.rs` 内定义唯一边界。搜索读取宿主的实时网络设置（`luaskills_bridge::live_network_settings`），保存新的 Hub 地址后立即切换，无需重启。
- **按语言查询**：`locale` 是界面语言代码，宿主只做字符集与长度的边界校验后原样作为 `lang` 传给 Hub，不在客户端匹配语言；省略时 Hub 返回原文（旧客户端行为不变）。每个结果可带 `content`（`kind` 为 `original`、`translation` 或 `original_fallback`，含本地化摘要、AI 简介与功能列表），未精炼的技能为 `null`。
- **详情 RPC**：`service_get_luaskills_hub_skill`（参数 `skillId` 必填、`locale` 可选）→ `luaskills_hub::get_official_hub_skill`，访问 `GET /api/v1/skills/{id}?lang=`，返回 `skill`（含请求语言的 `content`，只有译文节点带 `readme`）、`original`（原文节点，不带 `readme`）与 `latest`（已发布版本及其原文 README）。详情响应上限 `HUB_DETAIL_RESPONSE_MAX_BYTES`（2 MiB）与搜索上限同在 `luaskills_hub.rs` 唯一定义。
- **界面**：WebUI 安装对话框默认来源为 LuaSkills Hub，搜索选择后按技能标识安装；结果卡片显示本地化摘要并为译文标“译”，详情抽屉复用会话 Markdown 渲染 README，并可在译文与原文之间切换；界面语言变化时重新查询。TUI 在 LuaSkills 页提供搜索表单、逐项安装、详情与下一页按钮，详情以 Markdown 行显示 README 并提供同样的原文切换，镜像同一 RPC。
- **更新检查**：`service_check_luaskill_updates`（参数 `skillIds`，可选；缺省检查全部受管技能）按每个技能记录的安装来源查询最新版本，返回逐项结果，失败写入该行 `error`，不获取管理锁、不修改任何文件。ROOT 层技能走 System 权限的检查入口。WebUI 技能列表提供“检查更新”“全部更新”（逐个执行，首个失败即停止）与可更新标记。
- **更换来源**：`service_update_luaskill` 的 `sourceType` 与 `source` 必须同时给出或同时省略；给出时以 `replace_source: true` 更新，技能标识不变，只替换已记录来源。WebUI 在受管技能的操作菜单中提供“更换来源”，执行时复用更新进度弹窗。

### 13.7 网络来源：Hub 站点、GitHub 镜像与令牌

- **即时生效**：下载开关、GitHub 下载/API 基址与 Hub 地址保存后由 `save_luaskills_runtime_settings` 在引擎写锁内直接调用 `set_network_options` 应用，不需要重启。只有私有清单策略（`enable_private_url_skill_install` 与允许列表）仍由 `LuaSkillsNetworkSettings::requires_engine_restart_from` 判定为需要重启。
- **镜像目录**：内置镜像唯一来源是 LuaSkills 的 `builtin_github_mirrors`，宿主经 `service_luaskills_github_mirrors` 原样投影（`kind` 为 `direct`/`prefix`，`region` 为 `china`/`global`）。LuaSkills Hub 是独立发布体系，技能包下载地址由 Hub API 返回，不是 GitHub 镜像，不进入镜像目录。用户镜像保存在 `network.customGithubMirrors`，保存时由 `luaskills_network::validated_custom_github_mirrors` 校验标识与名称并用上游 `normalize_github_mirror_base_url` 规范化地址；标识不得与内置镜像重复。镜像失败不自动换用其他镜像。
- **测速**：`service_probe_luaskills_github_mirrors`（参数 `targets`，每项 `id` 与可空 `githubBaseUrl`）调用上游 `probe_github_mirrors`，样本路径、字节上限与超时使用上游默认值，宿主不另设边界。选用镜像只写入下载基址；GitHub API 基址仍是独立的高级设置。
- **GitHub 令牌**：只存放在操作系统凭据库（服务 `vulcan-code.luaskills.github-token-v1`，账户 `default`），绝不进入设置文件、应用数据库、日志或服务响应；目录只投影 `githubTokenConfigured`。`service_set_luaskills_github_token`（参数 `token`，`null` 表示清除）在管理锁下写入凭据库并立即应用到引擎；启动时读取失败只记录英文日志并以无令牌继续。令牌只由 LuaSkills 发给官方 GitHub API。
- **账号区域提示**：已登录 OpenVulcan 账号的区域（`china`/`global`）与当前官方 Hub 站点不一致时，WebUI 在技能列表与“Hub 与下载源”页提示切换。用户确认才写入 Hub 地址；选择保留则把该区域写入 `network.dismissedHubRegionPrompts`，此后不再为该区域提示。自定义 Hub 不提示。

## 14. 可延后项的结论与后续入口

方案「本地验证矩阵」第 6 条要求可延后项**分别记录结论和后续入口**，并规定「不得在报告中称这些能力已经修复」。本节即该记录；每项给出归属、取证、结论与后续入口。

### 14.1 安装分阶段接口（方案四.1）

- **取证：** `install_luaskill` 在入口取 `engine.write()`；`LuaEngine::install_skill_in_root` 是同步方法，内部 `apply_skill_request_in_root` 依次完成下载、校验、解压与提交，期间无 `await`、无线程派生。写锁因此**跨越整个网络 I/O**。
- **当前分阶段限制：** `SkillManager` 已公开 prepare、commit 和 rollback，可外部持有不透明事务；但安装 prepare 已将包重命名至最终目录，更新 prepare 已备份旧目录并发布新目录。因此它不是“不改变公开目录、随后才取得引擎锁应用”的纯暂存接口。当前 `LuaEngine` 同步入口还需一起协调目录重载、提交和回滚，不能仅据部分字段私有推断公开事务完全不可驱动。
- **结论：** 按方案四.1 的保留分支处理——**保留写锁**，并把安装期排队**作为明确限制显示**（安装进度横幅与更新进度弹窗在 `inFlight` 期间披露其他技能操作需要等待）。**该能力未修复，只是被如实呈现。**
- **更新进度交互：** 点击更新后立即打开弹窗，首个服务事件到达前显示准备状态；随后按 `correlationId` 排除点击前保留的旧操作，展示当前阶段、`percent` 与下载字节数。上游 `host/callbacks.rs` 仅在已下载字节和正数总量均已知时计算下载百分比，解析、解压等无数值阶段显示不确定进度；下载达到 100% 不代表整次更新完成。服务操作与本地请求结束后保留完成或失败结果，供用户关闭，关闭后的同一终态不会再次弹出。
- **后续入口：** 若改造普通技能安装，需先设计不提前改写公开目录、且与 `LuaEngine` 目录和代次一致的暂存提交边界；本轮不实施该独立改造。

### 14.2 插件租约逻辑目录

已由正式 LuaSkills 0.5.9 实现并接入，具体契约与证据见 §12。System 租约统一采用逻辑目录，不保留全局锁回退。普通文件型 runLua 的独立限制见下一项。

### 14.3 文件型 runLua 工作目录虚拟化（方案四.3）

- **取证：** `runlua.rs` 的文件执行路径仍取得 `lock_runlua_cwd_guard` 并切换进程目录；普通 Public 会话由 `lease.rs::eval_lua_value_with_optional_cwd` 保留同类行为。第 1.3 节回读没有将这些入口迁移成 System 逻辑目录。
- **结论：** 未实施。逐 VM 工作目录解析只能在其与原行为在文件读取、模块加载、子进程 cwd、失败、取消和跨工作区隔离上**全部等价**后才可考虑；当前**保留有界串行**。
- **后续入口：** 需先提供改前行为样本集作为等价性判据。

### 14.4 插件 lease 源码与编译缓存（方案四.6）

- **历史范围：** 原“未分开测量磁盘读取与 `load` 编译、未实施缓存”的结论仅描述旧 System JSON lease 路径，不能用于否定当前正式插件运行时。
- **当前正式路径：** 上游 `embedded/module.rs` 保留不可变模块声明并一次编译 JSON 契约；`engine/lease/embedded.rs` 在每个 VM 初始化时从该声明加载源码并捕获绑定函数。它没有跨 VM 共享 Lua 闭包，也不是 Lua 字节码缓存。宿主按可信包身份与内容代次构造定义，细节见插件执行契约。
- **后续入口：** 若新增字节码缓存或改变旧路径，仍须先测量，并证明版本、权限与沙箱隔离等价；不得跨 VM 复用绑定了环境的闭包。

### 14.5 Managed IO 外部覆盖策略（方案五.2）

- **归属：** Managed IO 的发布规则由**上游**实现。Vulcan 只通过 `capabilities.enable_managed_io_compat`（默认 `true`）传递开关，见 `luaskills_bridge.rs` 与 `luaskills_runtime.rs`。
- **当前规则：** 上游 `managed_io.rs` 的写入／更新模式在文件代次改变后按完整权威缓冲截断并覆盖外部改写；仅追加模式使用待写后缀追加。上游已有外部替换和无新写恢复测试，不能继续将覆盖策略标为尚待确认。
- **宿主边界：** Vulcan 没有声称自动合并外部修改的产品路径，本轮没有改变该策略，也没有新增宿主现场 IO 验收。需要保留外部修改的业务必须显式声明冲突或约束，不能把源码中的已有测试作为本轮真实 UI 验收。

### 14.6 受管运行时缓存失效扩展测试（方案五.3）

- **归属：** 计划缓存位于上游 `runtime/managed_package.rs:285-669`（`managed_runtime_plan_cache`、`managed_runtime_plan_cache_hits`、`cached_/cache_/evict_managed_runtime_plan`），**全部 `pub(crate)`**，宿主不可触及。
- **结论：** 「相同 owner 命中、锁文件或清单改变后失效、不同 owner 隔离」是**上游内部行为**，只能在上游内验收。Vulcan 侧已验收的部分是受管运行时**安装目标与清单校验**（既有测试覆盖严格目标、清单不匹配、版本意外与平台矩阵）。
- **已有覆盖：** 上游 `python_env_plan_resolves_manifests_and_lockfile` 已验证命中、锁文件失效、不同 owner 和释放；`node_env_plan_includes_package_manifest_hash` 已验证命中与清单失效。旧对照与候选的该测试文件相同。Vulcan 不重复实现私有缓存；这些源码断言不代表本轮宿主现场或最终候选多平台已经重新验收。

### 14.7 共同边界

本节分别保留旧方案延后项、已确认上游规则和当前正式插件路径，不能把历史“未实施”扩大为当前全部插件能力缺失，也不能根据源码或已有测试存在推断本轮普通 runLua、离线缓存、Managed IO 或真实界面验收全部通过。

## 15. Agent 请求期线程入场审计（方案四.4）

方案四.4 要求「对 Agent 每次请求新建 OS 线程的入口增加按调用类别区分的有界入场」。本节记录入口审计、类别差异与设计边界。

### 15.1 真实入场类别与容量来源

本任务约束的是 `vulcan-agent-runtime::luaskills_runtime` 在异步调用包装中为 LuaSkills 阻塞操作创建的 OS 工作线程。此前把 `agent_runtime/conversation_hooks` 的 VMM 会话钩子列为本节四个入口，是一次范围识别错误；它们不是 LuaSkills VM 池的借用入口，本次没有修改其调度语义。

LuaSkills 相关工作线程按上游实际资源拆成三类：

| 类别 | 宿主入场容量 | 上游对应资源 |
| --- | --- | --- |
| 普通技能调用 | `runtime_settings.vm_pool.max_size` | 主 VM 池 |
| 一次性文件型/内联 runLua | `runtime_settings.run_lua_pool.max_size` | 隔离 runLua VM 池 |
| 公开持久 runLua 会话操作 | 直接引用上游 `MAX_RUNTIME_SESSION_LEASES_PER_MANAGER` | 公共 Runtime Session manager 的租约上限 |

公开持久会话没有复用一次性 runLua 默认上限 4。Vulcan 的会话入场器在 `luaskills_runtime/types.rs::from_shared_engine` 中直接引用上游 `MAX_RUNTIME_SESSION_LEASES_PER_MANAGER`，保持与上游容量一致；不维护另一份宿主兼容常量。该入场器仅用于公开会话入口，独立 System 接口及正式插件父级不与它混用。

### 15.2 入场与取消语义

所有三类调用都在 `std::thread::Builder::spawn` 之前异步获取其独立 `tokio::sync::Semaphore` 许可。等待许可期间只有异步任务排队，不创建 OS 线程；若调用方在排队时取消，许可 future 随任务销毁。

取得许可后，许可所有权被移入工作线程，并在该线程真正退出时释放。调用方在工作已经开始后取消，只会丢弃结果等待者，不会提前释放许可，也不会允许额外线程突破上限。等待 5 秒、30 秒及之后每分钟输出一次带调用 ID、操作与阶段的诊断；这不把排队伪装成死锁。

### 15.3 验收结果

- `host_admission_bounds_workers_and_preserves_worker_owned_permits` 确定性验证两路入场、排队取消与工作线程持有许可；
- `real_skill_runlua_and_system_calls_overlap_inside_host_callbacks` 使用真实普通技能、一次性 runLua 与 System lease 路径验证并行边界，并在 8/16/64 路压力下检查线程峰值有界及完成后回落；
- 公开会话容量测试验证 8 条租约可创建，第 9 条由上游返回 `lease_limit_exceeded`；
- 数据库 4 worker / 64 槽与 `try_send` 饱和语义保持不变，因为等待 VM 的调用尚未进入数据库回调，不能把池等待机械换算成数据库负载。

### 15.4 当前限制

文件型 runLua 和带 `cwd` 的公开 Public 会话仍通过进程级 `lock_runlua_cwd_guard` 串行修改进程工作目录；上述入场只约束线程数量，不宣称消除了这条 cwd 串行。上游 `lease.rs::eval_runtime_session_with_profile_json` 对 System profile 直接在逻辑目录 VM 中执行，只有 Public 分支调用 `eval_lua_value_with_optional_cwd`。正式插件父级的用途容量和执行策略独立于这三个普通技能入场器，见第 8 节及插件执行契约。
