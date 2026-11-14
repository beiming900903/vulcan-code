# 开发环境初始化与 Cargo 并发策略

## 固定 Rust 工具链

仓库根 `rust-toolchain.toml` 固定 Rust `1.99.1`，使用 `minimal` Profile 并安装 `clippy`、`rustfmt`。根工作区采用 Cargo Resolver 3，49 个成员全部通过 `rust-version.workspace = true` 继承同一 `1.99.1` 支持基线；本仓库不再声明或测试更低 MSRV。

进入仓库后 Rustup 会按该文件解析工具链。不得使用 `--ignore-rust-version` 绕过基线，也不得只修改部分成员版本。`make check-deps` 会同时验证 Resolver、固定频道、必要组件、最小 Profile、成员继承和 Cargo 元数据的实际解析结果。

固定工具链不等于跨宿主运行验收。Windows x86_64 在本轮完成全量验证；Linux x86_64、Linux ARM64、macOS x86_64 与 macOS ARM64 必须后续在各自宿主独立运行同一治理、构建和测试入口。

## 构建模式与本地服务端口

数据库引擎随 VulcanCode 进程内建运行，不再有独立的控制器进程、端点或配置文件。构建与运行脚本因此不再按 Debug/Release 改写任何数据库端点：`make build`、`make br`、`make run`、`make release` 及其 `release` 变体在这一点上行为一致，也不存在需要保留或迁移的 `output/config/controller.json`。原先的 VLDB 端口 `19802`（Debug）与 `19801`（Release）已随该配置一并移除。

Vite 仅在 Debug 开发期间使用 `17601`，不参与数据库端口选择。VulcanCode WebUI 服务使用 Release `17891–17920`、Debug `17921–17950`；Release 不配置 Vite 端口。当前受支持的 WebUI 运行环境是浏览器访问（Vite 开发服务器或已构建资源）与 `apps/vulcan-code-desktop` Electron 桌面壳。Core 使用独立的 `tray-icon` 组件、Tao 原生事件循环和 `notify-rust` 故障通知，不依赖 Tauri、Wry 或 WebView2；Windows 可执行文件资源由 `winresource` 嵌入。WebUI 目录中保留的旧 Tauri 兼容适配器不是 Core 构建依赖，也不属于当前支持的宿主。内置 VMM 直接运行在 VulcanCode 进程内，不占用独立服务端口；外置 VMM 的端口完全来自用户配置。Windows 使用 PowerShell 构建与运行脚本，Linux x86_64、Linux ARM64 与 macOS 使用对应 Unix-like 脚本执行相同的构建规则。

> **关于 `protoc`。** `.cargo/config.toml` 把 `PROTOC` 与 `PROTOC_INCLUDE` 指向仓库随附的 `protoc`，使那些构建脚本直接调用 `protoc` 的第三方 crate（例如 `lancedb` 引入的 `lance-encoding`）无需系统级 protobuf 安装即可构建。通过 Cargo 的 `relative = true` 从仓库根解析路径，因此检出目录可以移动；Windows 启动命令时自动补充 `.exe`。显式设置的环境变量仍优先。

## 终端启动服务与 TUI

- `make service`（或 `make service release`）：在当前终端前台运行已经构建的 Core，固定使用已有的 `--headless` 模式，不创建托盘、Electron 或 Vite。它不编译；需要先完成 `make init-system`、`make sync` 和对应模式的完整构建。
- `make bt`（或 `make bt release`）：完成与 `make build` 相同的全部构建及打包，再启动后台无头 Core、等待发现文档与 `/health` 身份一致且状态为就绪，最后在当前终端进入 TUI。构建或启动失败时不进入 TUI；它不调用桌面或 Vite 的启动入口。
- TUI 使用 Core 的 `--describe` 所提供的数据目录绑定 `--service-root`，不猜测用户目录或服务端口。既有服务已经就绪时复用；启动中则等待，不竞争启动第二个进程。退出 TUI 不会停止后台 Core。新启动 Core 的诊断文件保存在 `target/core-cli/`，发现文档及其令牌不输出到命令行。

### 注册为用户服务（Linux）

`<发行目录>/bin/vulcan-code-service --install-user-service` 把无头 Core 注册为 systemd 用户单元（Release 为
`vulcan-code.service`，Debug 为 `vulcan-code-debug.service`，两者数据目录分离），设为随登录会话启动并立即启动，
等到发现文档就绪后输出 WebUI 地址。`--uninstall-user-service` 停止并取消启用该单元、删除单元文件；未安装时为成功的
空操作。两个参数都必须单独使用，执行完即退出，不启动服务业务。

- 只能从带有匹配构建模式 `runtime-layout.json` 的完整发行目录注册；单元写入 `$XDG_CONFIG_HOME/systemd/user`
  （缺省 `~/.config/systemd/user`），`ExecStart` 指向规范化后的可执行文件，并以 `--headless --no-debug-console` 运行。
- 单元记录安装时终端的 `PATH`，使智能体执行的命令找到与该终端相同的工具；更换工具链后重新安装即可刷新。
- `Restart=on-failure`：从 WebUI 或信号正常退出（退出码 0）不会被重新拉起，只有故障退出才重启。
- 安装前若已有其他进程（托盘、`make bt` 拉起的 Core 或终端服务）持有同一数据目录，直接报错，不让单元启动后立即
  以“已被占用”退出；重新安装会先停止已注册的单元再替换。
- 默认随登录会话存活；需要注销后继续运行时由用户自行执行 `loginctl enable-linger`，安装命令不修改该系统设置。
- 服务运行后，`bin/vulcan-tui` 与浏览器访问 WebUI 都直接连接该实例；`make bt` 也会复用它，而不再启动第二个 Core。
- Windows 与 macOS 暂不支持注册，命令会明确报错。

普通完整构建不承担停止正在运行的发行物这一职责，Unix 下仍需先退出需要替换的发行实例；需要只更新 Core 时使用下述增量重启入口。`make run` 保持启动 Electron 的原意，不等同于纯服务启动。

GNU Make 在 Unix 下通过 `make.sh` 分发构建及运行；直接执行 `pwsh -File make.ps1 build/run/service/br/bt/core-build/core-restart` 时也显式委托同一 Unix 入口，不会执行面向 Windows 的 Core 生命周期脚本。依赖初始化中原本共用的 PowerShell 安装器继续保留。

### Debug 与 Release 的发行目录

Debug 与 Release 各自拥有独立的同级发行目录，使两个版本可以同时运行，并允许用已运行的 Release 版调试正在开发的 Debug 版：

| 模式 | 发行目录 | 构建并运行 | 仅同步资源 |
| --- | --- | --- | --- |
| Debug | `output/` | `make br` | `make sync` |
| Release | `output-release/` | `make br release` | `make sync release` |

目录名只在 `scripts/desktop-runtime.mjs` 中定义一次，其余入口（构建、同步、运行、发布、打包、TUI）一律通过 `desktop-runtime.mjs root <debug|release>` 取得，不自行拼接路径。未知模式直接报错，不回退到 Debug 目录。`VULCAN_DESKTOP_DISTRIBUTION_ROOT` 仍可把发行目录整体改到 `target/` 之内，用于发布验收，此时它覆盖两个模式的默认目录。

发布前的进程释放（`Prepare-DistributionPublication`）只停止可执行文件位于本模式发行目录内的进程；`output-release/` 与 `output/` 互为同级而非嵌套，`make br` 不会停止 Release 版，`make br release` 也不会停止 Debug 版。两个版本的用户数据根与默认端口本来就按 `cfg!(debug_assertions)` 分开，因此同时运行不会争用同一份数据或端口。

`make init-system` 为两个发行目录分别准备受管 Node/pnpm。`make clean` 与 `make clean release` 只清理 Cargo 编译产物，两个发行目录都不触碰。`make core-build`、`make core-restart` 只针对 Debug 发行目录。`output-release/` 与其 `.previous`、`.rejected-*` 备份同样由 `.gitignore` 忽略，且与 `output/` 一样不得手工编辑。

### 清理编译产物

`make clean` 仅执行 `cargo clean --profile dev`，清理 Debug 编译产物；`make clean release` 仅执行 `cargo clean --profile release`，清理 Release 编译产物。`release` 在此是模式参数，不会触发构建。Windows、Linux、macOS 共用相同语义；目标目录与目标三元组由 Cargo 根据环境变量及 `.cargo/config.toml` 解析，不手工猜测目录。清理不依赖 Node、pnpm 或下载依赖，不触碰 `output/`、`output-release/`、`third_party/`、用户数据及另一模式的编译产物；它不会停止运行中的应用，文件被占用时保留 Cargo 的错误并退出。

`make prune` 与 `make prune release` 不经构建，把对应模式目录清理到每个工作区 crate 最新的若干单元与增量目录，第三方单元不动；`make test`、`make check` 结束后自动对 Debug 执行一次，其他 Cargo 命令正占用模式目录时直接跳过。规则与上限 `KeptUnitsPerCrate` 定义在 `scripts/cargo_artifact_cleanup.ps1`；实现为 PowerShell，Unix 上需要 `pwsh`。make 构建后的清理同样受这一上限约束：每个工作区 crate 在保留期内也只保留最新的若干个未用单元与增量目录。

下载的构建输入统一保存在 `third_party/`，具体目录由 `scripts/build-input-paths.mjs` 定义。`output/` 与 `output-release/` 仅保存运行与分发副本，可以重新同步及打包恢复。直接运行无参数 `cargo clean` 仍会清空整个 Cargo 目标目录，但不会删除这些持久输入。旧的 `target/browser-runtime/`、`target/build-input-cache/` 与 `target/electron-runtime-cache/` 不再作为下载源；切换入口前可显式复制到对应的新目录并核对完整性，脚本不会探测或静默回退到旧路径。

### LanceDB 原生库

LanceDB 及其依赖闭包（lance、datafusion、arrow）只编译进 C 动态库 `vulcan-lancedb-native`
（Windows `vulcan_lancedb_native.dll`，Linux `libvulcan_lancedb_native.so`，macOS
`libvulcan_lancedb_native.dylib`）。服务通过 `vulcan-lancedb-engine` 在首次使用时从自身可执行文件所在目录
加载它；可执行文件位于 Cargo 的 `deps/` 时（测试二进制）改从上一级 profile 目录加载。三个 crate 的分工：

| crate | 内容 |
| --- | --- |
| `vulcan-lancedb-contract` | 公开类型、错误、配置、库锁与线协议，不依赖 LanceDB |
| `vulcan-lancedb-native` | `cdylib`，持有引擎、运行时与存储格式，导出 C ABI |
| `vulcan-lancedb-engine` | 客户端，加载库并保留原有公开路径 |

线协议版本由 `vulcan-lancedb-contract` 的 `wire::PROTOCOL_VERSION` 唯一定义，库与客户端版本不一致时
直接报 `LibraryUnavailable`，不降级。依赖不变时修改服务代码不会重新编译或链接 LanceDB。

- 分发清单 `scripts/shipped-rust-packages.json` 以 `library` 条目登记该库；完整构建、Unix 发布与
  `make core-build`/`make core-restart` 都把它放进服务所在的 `bin/`。
- 该库始终在独立的 Cargo 调用中构建（`shipped-rust-packages.mjs cargo-args library`），绝不与服务同批。
  服务被排除在 Hakari 遍历之外，其托盘等私有特性会改变共享依赖（如 `windows-sys`、`tracing-core`）的特性集；
  同批构建会编译出另一份 LanceDB 变体，使完整构建与 `make test` 各自重编 LanceDB。
- 测试构建不会生成 `cdylib`，`make test` 因此先执行 `cargo build --locked -p vulcan-lancedb-native`。
  直接使用 `cargo test`/`cargo nextest` 跑依赖知识库或插件存储的测试时，需要先执行同一命令，否则这些测试按
  库不可用失败。
- 编译产物清理把该库的指纹单元视为本次构建使用过，不会扫除未变化的库。

## Windows Debug 下只更新 Core

应用 SQLite 结构变化必须同时修改当前 DDL 并在 `APP_SCHEMA_MIGRATIONS` 末尾追加一步，版本由升级链长度派生，
启动时逐版本升级。开发期在同一版本内直接改动 DDL 留下的结构漂移，在启动新 Core 前独立执行 Debug 数据库检查
与同步：维护命令直接复用生产 DDL，先备份、事务更新，再校验保留值及完整性；不会随 `make build`、
`make run`、`make core-restart` 或 Release 启动隐式执行。升级链、具体入口和可处理的结构差异见
[应用数据库升级与本地 Debug 维护](debug-database-maintenance.md)。

### 内置浏览器依赖

“Vulcan 浏览器”保留插件设置入口，作为随包交付的内置组件。官方 MCP 固定为 `0.0.82`，其 Playwright 与 playwright-core 固定为 `1.64.0-alpha-1789764292000`；版本组合在 `scripts/browser-runtime.mjs` 中校验，不使用 `latest`。升级必须同时更新锁文件并重新验证 MCP 导航、快照、点击、截图及授权撤销。

先执行 `make init-system` 准备受管 Node `24.18.0` 和 pnpm `11.11.0`，再执行 `make sync`。这两个入口都会通过 `scripts/managed-node.mjs` 在 `third_party/managed-node/` 补齐缺失的受管 Node/pnpm，使用官方精确版本地址并校验 SHA-256/SHA-512，再复制到对应发行目录的 `dependencies/runtimes/node/`（Debug 为 `output/`，Release 为 `output-release/`，`make sync release` 同步后者）；删除发行副本后可从持久输入离线恢复，不依赖已有 `output/browser` 或正在运行的 Core。同步阶段按锁文件准备插件依赖，并把对应 Chromium 下载到仓库 `third_party/browser-runtime/<Playwright版本>/<系统-架构>/`；`make build` 仅使用已准备、已同步的输入，复制 Chromium 并生成 `output/browser`，缺失时失败，不在运行期间下载。Electron 构建下载缓存单独保存在 `third_party/electron-runtime-cache/`，不与 Chromium 混用。系统 Node/pnpm 仍属于下文规定的开发机前置工具。

发行物中 `desktop/runtime/` 仅运行 Electron GUI；`browser/runtime/` 保存 Core 浏览器宿主及驱动，`browser/chromium/` 保存独立 Chromium，宿主由 `dependencies/runtimes/node/` 的受管 Node 启动。运行布局架构为 3，`browserHostExecutable` 与 `desktopExecutable` 分别声明两个程序；`tuiExecutable` 声明与服务同目录发布的终端客户端 `bin/vulcan-tui`，Windows 托盘的“打开终端客户端”只按该条目启动。旧版用户目录下的浏览器下载不会覆盖随包版本，也不会在启动时自动删除。

用户机器获得 Playwright 推荐的完整组件。同步阶段的 `install chromium --no-shell` 在缓存中为当前平台放入 Chromium 及其附带组件（录屏用的 ffmpeg，Windows 上另有 winldd）；打包时 Chromium 进入 `browser/chromium/`，其余组件原样进入 `browser/runtime/node_modules/playwright-core/.local-browsers/`，不在仓库内复述组件清单。浏览器宿主以首个导入的 `chromium-host/playwright-registry.ts` 把 `PLAYWRIGHT_BROWSERS_PATH` 固定为 `0`（Playwright 的自包含模式），因此组件只从随包目录解析，用户自己的 Playwright 缓存或继承的环境变量不能替换它们。打包阶段用受管 Node 与随包运行时按同一模式实际录制一段视频（`scripts/browser-video-check.mjs`），失败即停止；`browser/chromium/package-manifest.json` 的文件清单同时覆盖这些组件，迁移后的发行物可发现载荷不完整。Linux 的系统库、字体与 Xvfb 由发行版包管理器提供，不随包分发：用户执行 `vulcan-code-service --install-browser-dependencies`（必须单独使用），它从宿主入口目录解析随包 playwright-core，运行其 `install-deps chromium`，由 Playwright 按发行版本选择软件包并自行请求 sudo。Windows 与 macOS 无需此步骤，执行时直接报错说明。

首次启用只检查本地依赖，并在使用时发布插件私有执行代次，无须下载。组件缺失或损坏时修复应用安装；开发环境重新执行 `make sync` 和完整构建。Node、Electron 与 Chromium 的上游许可文件随对应目录保留。

首次启动或修改 Electron 壳、运行布局时，先执行 `make build`，再执行 `make run`。仅修改 WebUI 页面时，保留 Vite 与桌面壳运行即可，页面由 Vite 热更新。

Windows 下需要完整重建并重新启动时使用 `make br`（发布模式为 `make br release`）。全部编译成功后、写入发行目录之前，它会强制停止可执行文件属于当前发行目录的进程，包括核心、桌面壳、终端宿主和内置浏览器，等待退出后发布并启动新桌面；其他发行目录及外部 Vite 不在停止范围内。普通 `make build` 不自动停止程序，发现发行目录仍有进程时在发布前明确拒绝。强制重启会中断运行中的任务；需要先排空时可先从托盘退出服务。

仅修改 Rust Core 时，不必重新构建或关闭桌面壳。日常直接执行：

```powershell
make core-restart
```

只想验证编译而暂不重启时，使用 `make core-build`；它只把 `vulcan-code-service` 与 LanceDB 原生库编译到 Cargo 的 Debug 目录，不替换正在运行的服务。`make core-restart` 自带增量编译，使用桌面运行入口共用的布局校验；编译和启动契约校验成功后，自动强制停止精确路径匹配的当前 Debug Core，替换 `output/bin/vulcan-code-service.exe` 及同目录的 `vulcan_lancedb_native.dll`（两者一同备份、一同恢复，不混用不同代次）并启动新 Core，无需手动退出托盘。其他发行目录或 Release 进程不在停止范围内；如果其他程序占有同一 Debug 数据目录，则拒绝重启。桌面壳与 Vite 保持运行，WebUI 断线后按新 Core 的发现信息重连。重启期间正在执行的 AI 请求会中断，应在重连后重新发起；需要优雅排空时可先自行从托盘退出服务。

上述托盘及可见调试控制台行为属于 Windows Debug。修改 `vulcan-terminal-host`、Electron、运行布局或 Core 与壳之间的启动协议时，使用完整 `make build`；修改权威运行资源时，先显式执行 `make sync`。`make core-restart` 不会同步资源；新 Core 退出且尚未健康就绪时，恢复原可执行文件备份。运行布局架构版本由 `scripts/desktop-runtime.mjs` 定义并校验，核心脚本不再维护独立版本数字。

核心重启保留可见的 Debug 控制台，不再隐藏窗口。宣布成功必须同时满足新进程健康身份匹配、托盘事件循环就绪以及 Windows 确认托盘注册；本次进程通过启动器指定的 `VULCAN_CORE_STARTUP_REPORT` 写入独立确认文件，位于 `target/core-restart/`。该变量只用于重启确认，不替代服务发现。Windows 可以将已注册图标放在折叠区域；图标外观改变不意味着服务未启动。关闭工作台窗口仍只结束桌面客户端，后台核心退出使用托盘菜单。

### Linux / WSL 的 Core 增量更新

Linux 的 `make core-build` 直接使用 Node 启动 `cargo build --locked -p vulcan-code-service --message-format=json`，再以独立调用构建 `vulcan-lancedb-native`，只接受本轮 Cargo 报告且成功生成的唯一服务制品与唯一库制品，不从固定 target 路径挑选旧文件，不要求 PowerShell 或 pnpm。

`make core-restart` 要求已有匹配宿主、架构及 Debug 模式的完整发行物。它先编译并比较新旧 `--describe` 启动契约，复制并核验新制品摘要，再通过发现 PID、Linux `/proc/<pid>/exe` 和进程用户核对现有持有者。只向当前发行物的 Core 发送 `SIGTERM`；外部目录的实例或无法确认归属的进程不会被终止。旧持有者未能在限定时间内退出时，不替换程序。

同目录暂存及原子改名用于发布新 Core 及其 LanceDB 库，旧文件各自保留为独立 `.previous` 文件直到无头服务就绪。新服务启动失败且已安全退出时，恢复并重新启动旧程序；无法安全关闭或恢复时保留备份并明确报告路径。Electron、Vite、TUI 和同步资源不重建。该原生重启入口目前明确支持 Linux，Windows 保持既有托盘重启入口，macOS 不冒称已支持增量重启。

## Core 原生托盘边界

托盘实现位于 `apps/vulcan-code-service/src/tray.rs`，仅在 `desktop-tray` 特性下编译。`tray-icon` 负责图标与菜单，Tao 仅负责主线程的操作系统事件循环，不创建工作台窗口；窗口继续由 Electron 提供。这样托盘无需引入 Tauri 应用框架、WebView 运行时、插件注册或前端 IPC。

菜单项、图标及其事件分发始终留在主线程。退出菜单启动独立排空线程，排空成功后通知主线程先释放托盘，再结束事件循环；排空失败则恢复服务准入并保留托盘。服务所有者随事件循环保留，不再通过手动泄漏延长生命周期。启动确认继续检查 Windows 实际注册的托盘矩形，不以 HTTP 就绪代替托盘就绪；Debug 控制台继续使用独立的原生控制台模块。

Windows 图标与版本资源由 `winresource` 嵌入。故障通知直接调用 `notify-rust`；其 Windows 后端包名 `tauri-winrt-notification` 是独立的 WinRT 通知封装，不包含 Tauri 应用框架。Linux 原生菜单仍使用 GTK3，其宏依赖 `proc-macro-error 1` 的现有停维护公告保留为 `deny.toml` 中唯一显式例外；升级 Tao 或 `tray-icon` 时必须重新检查此依赖链。

依赖移除后的验证包括 Core 全目标编译、图标解码测试、服务启动参数与发现身份测试，以及 `cargo tree -p vulcan-code-service --target all --locked` 对 Tauri/Wry/WebView 的依赖排除检查。跨平台菜单外观和桌面通知必须在对应操作系统实际验收，不能用 Windows 编译结果代替。

## 新环境初始化

在已经具备 `make`、Rustup stable、PowerShell 7 以及 Node.js/pnpm 的环境中，仓库依赖统一通过一个入口初始化：

`make.sh` 与 `make.ps1` 在分发命令前共用 `scripts/check-build-environment.cjs` 检查宿主前置条件。系统 Node 与 pnpm 的最低版本派生自 `crates/vulcan-service-runtime/managed-runtimes.json`，不另写一份版本常量；需要 PowerShell 的入口同时检查 PowerShell 7。检查器使用旧 Node 也能运行的 CommonJS 和基础 API：服务器镜像自带旧 Node 时，会列出版本过旧、命令缺失或不可执行的具体问题，不先加载依赖 `fs/promises.cp` 等新版 API 的模块。仅安装 Rust 开发工具的 `make deps rust-tools` 不依赖 Node；`make help` 也不执行环境检查。

该检查只诊断，不升级系统软件或改写用户 PATH。先安装或激活合适的宿主工具，再执行以下初始化；初始化完成后仍需显式 `make sync`，最后才能构建。受管发行运行时的精确锁定版本与宿主工具的最低版本要求是不同概念，不能用存在 `output/` 目录代替宿主检查通过。

```bash
make init-system
```

该命令按固定顺序执行：

0. Linux 首先执行 `scripts/install_linux_system_deps.sh`，检查 GTK3/GLib/GObject/GIO、AppIndicator 或 Ayatana 的 `pkg-config` 元数据，并实际编译链接一个只验证 `libxdo` 开发文件的探针；不会运行探针。同时在 `ldconfig -p` 链接器缓存中查找随包 Chromium 在 glibc 之外直接加载的全部共享库（NSS、NSPR、ALSA、ATK、CUPS、GBM、X11 等），因为打包阶段会实际启动该 Chromium 读取版本，缺库时报 `error while loading shared libraries`。缺失时先显示具体探针错误，再通过 Debian/Ubuntu 的 APT 安装脚本中声明的开发软件包集合与 Chromium 运行库集合，随后复核。Chromium 运行库条目以 `|` 列出候选名，64 位 time_t 改名（如 `libasound2t64`）在前，刷新软件包列表后选用第一个有安装候选版本的名称，以同时覆盖新旧发行版；任一条目无法解析即在安装前停止。已有依赖满足时不请求管理员权限、不运行包管理器；普通账户可复用已缓存或免密的 sudo 授权。需要密码且输入与错误输出均连接终端时，初始化自动执行 `sudo -v` 请求 Linux 账户密码，成功后继续安装；拒绝或取消授权立即停止，不运行 APT 或后续初始化阶段。非交互环境不等待密码，明确要求在交互终端重新运行 `make init-system` 或由管理员准备依赖。只有系统包操作提升权限，Rust 工具与其他项目依赖仍由当前账户安装。其他发行版缺少依赖时要求先通过本机包管理器补齐；非 Linux 宿主跳过此步骤。运行库存在或桌面可以启动，不能替代开发头文件及 `.pc` 元数据。`make.sh` 和直接运行的 `make.ps1 init-system` 共用这个入口。
1. 以 Cargo 并发度 2 安装缺失的 `cargo-hakari`、`cargo-nextest`、`cargo-deny` 与 `cargo-about`，并将低于仓库最低要求的工具更新到兼容版本。
2. 安装宿主依赖与内建工具。
3. 安装 LuaSkills 运行时依赖；原生 VMM 在 VulcanCode 进程内运行，不需要独立运行时安装。
4. 取得 LanceDB 依赖闭包构建所需的随仓库 `protoc`（`protoc 31.1`）。`lancedb` 引入的 `lance-encoding` 会在其构建脚本中直接调用 `protoc`，因此没有它工作区无法编译；`.cargo/config.toml` 把 `PROTOC` 与 `PROTOC_INCLUDE` 指向 `.cargo/protoc/` 下的该副本，开发机无需系统级 protobuf 安装。安装器先按锁定归档摘要验证，再解压并核验版本及头文件，准备完成后才替换安装；准备失败保留现有安装。下载产物不符时直接报错，不静默接受。该目录属于构建输入，已被 git 忽略，因此不随仓库提交。

已满足版本要求的工具会被保留，不会重复安装。

类 Unix 初始化同样必须实际执行锁定的 `protoc` 安装步骤，成功后才准备受管 Node 并报告完成。`node --test scripts/check-build-environment.test.cjs scripts/install-linux-system-deps.test.cjs` 使用隔离子程序验证真实入口的阶段顺序、失败传递，以及 Linux 系统依赖就绪时跳过安装、权限不足时拒绝、安装后仍未满足依赖时拒绝等行为，不运行真实包管理器或联网；`pwsh -NoProfile -File scripts/test_dependency_archive.ps1` 使用真实离线归档验证共享解压规则与 Lua 安装器的解压、标记及失败保留行为。

Lua 运行时安装器通过 `scripts/dependency-archive.ps1` 使用共享解压规则：Windows 使用系统 `tar.exe` 或 PATH 中的同名应用，Linux/macOS 使用 PATH 中的 `tar`。原生 tar 返回非零退出码时立即失败，不继续发布文件或写入安装标记。RTK 与 sg/rg 一样由 `install-locked-tools.mjs` 按锁文件安装（`make deps tools`）；Linux arm64 上 RTK 从锁定的源码归档本机编译，需要已安装的固定工具链与 C 编译器（`init-system` 已提供），约增加数分钟。

macOS 运行时最低支持 15.0。`make deps lua` 解压已校验的 LuaSkills 归档后，由 `scripts/macos-lua-runtime.ps1` 把 `lua_packages/lib/lua` 下的原生模块改为加载器查找的 `.dylib` 后缀，并使用 Apple 工具将非系统依赖指向随包 `libs`，重新签名并回读实际加载命令。Lua cURL 显式链接 macOS 15 两种架构提供的 `/usr/lib/libcurl.4.dylib`，原归档中依赖 Homebrew 的 libcurl 副本不进入发行物。缺库、无法解析的加载路径或超过 15.0 的最低版本会中止准备。旧布局缓存从已校验归档重建。

`make sync` 和 `make sync release` 在复制前检查已准备的 macOS 原生库，替换发行目录中的旧原生库目录，并用本机工具链构建探针。探针通过随包 LuaJIT 从同步后的目录加载原生模块，验证 OpenSSL 随机数、AES-GCM 和 cURL HTTPS 句柄创建，不发起外部请求。更新 LuaSkills、官方归档或 macOS 支持下限时须复核这些加载规则和系统 libcurl 的 ABI。

macOS Core 静态链接的 LuaJIT 为随包原生模块提供 Lua C API。Core 的构建脚本显式保留两种架构的模块导入，发行前用 `nm` 核对实际 Core 导出和实际模块导入，缺失符号直接中止打包。更新原生模块时同步更新 Core 的符号清单。

Linux 与 macOS 共用 Core 的 Lua 符号清单。Linux 使用 ELF 链接参数保留并导出这些符号，发行归档前用 `readelf` 校验 Core 的动态符号表和各 Lua 模块的导入。发布作业将其 glibc 基线传给同一检查，Core 或随包 Lua 共享对象的实际 ELF 版本需求超出基线即中止打包。LuaSkills 上游 Linux 归档在 Ubuntu 22.04 构建，共享对象使用相对于自身位置的 `$ORIGIN` 路径查找归档内依赖。

Rust 工具已存在时会只打印版本并跳过安装，不会重复编译或覆盖现有工具。Windows 通过 `make.ps1` 执行，类 Unix 环境通过 `make.sh` 转发到同一套 PowerShell 依赖入口。

单独安装 Rust 工作区工具可以执行：

```bash
make deps-rust-tools
```

## Rust 依赖治理检查

### 持续集成中的精简准备

开发机继续使用 `make init-system`。GitHub Actions 的 Rust 作业（测试在 Ubuntu，另有 Windows 与 macOS 编译检查和手动的完整测试）只准备当前检查实际使用的依赖：`.github/actions/rust-setup` 安装 Linux 原生包、固定工具链与 protoc；测试作业再由 `install-locked-tools.mjs` 安装 nextest 与内置 sg/rg/rtk，执行 `make deps lua` 和 `managed-node.mjs output`（只验证 Debug 发行目录），完整测试在运行时回归前按受管运行时清单安装宿主 pnpm，并按构建输入锁准备判定模型运行库，再显式执行 `make sync`。Windows 使用系统 bsdtar 解压受管运行时归档，Git Bash 中的同名工具不会影响该路径。不为这些测试编译安装未使用的 hakari、deny 或 about。

nextest 的 CI 二进制只锁定 CI 使用的 Windows x64 与 Linux x64，安装到忽略目录 `target/ci-tools/bin/`，不覆盖开发机全局 Cargo 工具。唯一版本与归档 SHA-256 来源为 `scripts/build-inputs.lock.json`；下载器检查缓存摘要，安装器检查实际版本，不使用 latest 或未校验的归档。开发机其他平台仍使用原初始化入口。分类与缓存规则见 [仓库检查](repository-quality-gates.md)。

### 依赖治理入口

依赖版本、Feature 或成员清单变化后，执行仓库只读治理入口：

```bash
make check-deps
```

该入口检查 Resolver 与工具链政策、工作区版本权威、Hakari 生成状态、RustSec 公告、许可证、来源、第三方公告源码制品、格式和受限并发静态编译，不会自动升级依赖或改写 `Cargo.lock`。完整规则以仓库当前的 `deny.toml`、`about.toml` 与 `make check-deps` 实际实现为准。

## workspace-hack 依赖去重

仓库使用 `cargo-hakari` 生成根目录下的 `workspace-hack` crate，并将统一后的依赖特性集中到该 crate，减少多 crate 工作区中的重复依赖编译。相关配置位于 `.config/hakari.toml`，生成结果位于 `workspace-hack/`。

依赖声明或 feature 变化后，使用以下命令检查并刷新生成结果：

```bash
cargo hakari generate
cargo hakari manage-deps
cargo hakari verify
```

`.config/hakari.toml` 把服务与 `vulcan-lancedb-native` 排除在遍历之外：前者的托盘依赖不得扩散，后者的
LanceDB 专属特性（arrow 压缩、csv、json、ffi 等）不得统一进工作区。`vulcan-lancedb-native` 及其链接的
`vulcan-lancedb-contract`、`vulcan-file-ownership` 列在 `final-excludes` 中、不依赖 `workspace-hack`，
使工作区其他位置的特性变化不改变原生库的构建输入；`cargo hakari manage-deps` 不会把依赖加回。

`workspace-hack` 只能合并兼容的同版本依赖特性；不同主版本或无法安全统一的依赖仍会保留为多个版本。它不能替代 Cargo.lock，也不会把语义上不同的依赖强行改成同一版本。当前生成平台覆盖 Windows x86_64、Linux x86_64、Linux ARM64、macOS x86_64 与 macOS ARM64。

## 默认资源限制

正式服务日志的归属、轮转和过滤规则见 [服务诊断日志](service-diagnostics.md)。被忽略测试的分类、离线串行门禁、并发代码压力复现及外部验收前置条件见 [显式测试入口](explicit-test-gates.md)。

服务内核的开发模式采用根 `Cargo.toml` 中的包级 `debug = "limited"`：保留源码位置和回溯信息，减少完整类型及局部变量调试信息。原因是 Windows 实测完整调试内核归档达到 4,247,716,754 字节，符号索引已变为 `/SYM64/`，MSVC 报无效库格式并在服务链接时产生 49 个未解析符号；这些符号实际仍存在于归档中。该设置只影响内核包，测试模式继承开发模式，发布模式及其他依赖的设置保持原定义。需要完整局部变量调试信息时，应先解决内核归档体积问题，再覆盖此设置。第三方依赖另以 `[profile.dev.package."*"] debug = "line-tables-only"` 只保留行表：回溯仍含源码位置，链接时间与 PDB 体积随之下降；工作区 crate 不受此设置影响。需要单步进入第三方源码时，临时为该依赖单独覆盖为 `debug = true`。

根目录 `.cargo/config.toml` 设置 `build.jobs = 16`，限制 Cargo 编译进程并发；该值同时是 rustc 并行代码生成线程的上限，取 32 线程开发机的一半，为 IDE、服务与测试进程保留余量。同时以 `RUST_TEST_THREADS = 4` 约束常规测试 harness。`.config/nextest.toml` 将 nextest 默认测试线程限制为 4。

同一文件为 `x86_64-pc-windows-msvc` 指定 `linker = "rust-lld.exe"`（工具链自带，无需另行安装）：修改内核后重新链接约 500 MB 的调试服务可执行文件，MSVC `link.exe` 实测 27.7 秒，rust-lld 为 15.3 秒（2026-10-12）。Cargo 只接受精确目标三元组下的 `linker`，因此它不写在 `cfg(all(windows, target_env = "msvc"))` 表中。发布构建同样使用该链接器。

临时调整时可以显式覆盖：

```bash
cargo test --jobs 2 -- --test-threads=1
cargo nextest run --test-threads=1
```

涉及 Provider Manifest、服务内核注册表、端口或其他进程级共享资源的测试由 `shared-runtime-resources` Nextest 组串行执行。新增或重命名此类测试后，必须使用 `cargo nextest show-config test-groups` 核对实际匹配。工作区全量测试可能同时启动大量测试进程，应先执行目标 crate 的 `cargo check` 或定向测试。`cargo nextest` 不覆盖 doctest，文档测试需要额外执行：

```bash
cargo test --doc
```

如果确实需要临时提高并发度，应在任务记录中说明原因，并确认不会影响开发机响应性或共享资源测试的稳定性。

### 本地测试入口

本地与 CI 使用同一入口：CI 运行 `fast`，完整测试工作流运行 `full`（见 [仓库检查](repository-quality-gates.md)）。

| 命令 | 内容 |
| --- | --- |
| `make test` | 以 nextest `fast` 配置运行全工作区测试，跳过成本来自真实流逝时间、真实 VMM 控制器或完整插件生命周期的测试。 |
| `make test full` | 运行全部 nextest 测试、`scripts/test-isolated-callbacks.mjs` 中的进程隔离回调测试，以及 `cargo test --doc`。 |
| `node scripts/test-rust.mjs quarantine` | 只运行 `.config/ci-quarantine.json` 中已隔离的不稳定测试；前两项都排除这些测试。 |

三者都由 `scripts/test-rust.mjs` 执行，先构建 LanceDB 原生库（见上文），并继承 `.config/nextest.toml` 的线程上限与串行组。`fast` 的排除名单只在 `.config/nextest.toml` 的 `[profile.fast]` 中维护；新增长耗时测试时更新该过滤器，并用 `cargo nextest list --profile fast` 核对实际排除结果。合并或发布前执行 `make test full`。

### 手动验证夹具

下列脚本不属于 `make test`，只在修改对应模块时手动执行；它们都只使用合成数据或可丢弃进程，不触碰用户配置、数据库或正在运行的发行实例。修改对应生产代码后若夹具不再成立，必须同一任务中修正或删除，不能留下无法通过的脚本。

| 脚本 | 何时执行 | 执行方式 |
| --- | --- | --- |
| `apps/vulcan-code-desktop/tools/test-workbench-history.mjs` | 修改工作台初始载入或渲染器恢复；验证启动页不能被历史后退重新打开，独立浏览内容仍可后退 | 使用清单指定的 pnpm 安装桌面依赖，执行 `node scripts/install-electron.mjs`；桌面 `build` 后执行 `pnpm --dir apps/vulcan-code-desktop run test:workbench-history`，需要可运行 Electron 的桌面环境 |
| `scripts/test_core_debug_cycle.ps1` | 修改 Debug Core 强制重启归属逻辑 | `pwsh -File`，无参数 |
| `scripts/test_distribution_processes.ps1` | 修改发行目录发布与进程占用处理 | `pwsh -File`，无参数 |
| `scripts/test_cargo_artifact_cleanup.ps1` | 修改 Cargo 产物清理；会执行真实隔离 Cargo 构建，耗时较长 | `pwsh -File`，无参数 |
| `scripts/test_rust_third_party_notices_index.ps1` | 修改第三方声明索引器或 `THIRD_PARTY_NOTICES.md` | `pwsh -File`，无参数 |
| `scripts/test_context7_plugin.lua` | 修改 Context7 插件 Bundle | 在 Vulcan Lua 运行时中执行，传入 `plugin_path`；`online` 可选开启在线冒烟 |
| `scripts/test_statistics_cost_pagination.lua`、`scripts/test_statistics_page_projections.lua` | 修改用量统计插件的费用累加或分页视图 | 在 Vulcan Lua 运行时中执行，传入 `plugin_path`，`change_revision` 可选 |
| `scripts/test_vulcan_plugin_lua_modules.lua` | 修改 Vulcan Plugin 白名单 Lua 模块桥 | 在 Vulcan Lua 运行时中执行，传入 `bridge_path` 与 `workspace_root` |
| `scripts/sync-common-model-capabilities.mjs` | 由 Provider Manifest 重新生成公共模型能力目录 | `node`；默认联网取 OpenRouter 交叉参照，`--offline` 以已发布目录为参照，`--check` 只校验不写入；改写 `common_model_capabilities.json`，执行后审阅差异 |
| `scripts/audit_cliproxy_protocol_changes.ps1` | 审计 CLIProxyAPI 协议兼容差异 | `pwsh -File`；参数默认指向本机 CLIProxyAPI 检出与最近审核基线，可用 `-RepositoryPath`、`-BaselineCommit` 覆盖；只读上游仓库，改写 `docs/protocol-compatibility/cliproxy-audit-latest.md` |
| `scripts/plugin-runtime-validation/` | 历史证据：LuaSkills 0.5.9 → 0.6 迁移的性能对照，迁移已完成，不作为当前验证入口 | 见目录内 `README.md` |


## 锁定命令行构建输入

scripts/build-inputs.lock.json 是内建 sg、rg、rtk 和 protoc 的版本、各平台归档摘要及许可证摘要的唯一来源。安装入口共用 install-locked-tools.mjs，不查询最新发行版；升级必须评审清单差异并验证对应平台。此清单不替代 Cargo、pnpm 及已有受管 Node 的锁定机制。

同一清单的 `judge_runtime` 段锁定本机权限判定器的随包执行库：官方 ONNX Runtime CPU 核心库与 WebGPU 执行提供器插件（通用 GPU 路径，适配任意支持 WebGPU 的显卡）。`make init-system` 与 `make deps tools` 经 `install_bundled_tools` 依次安装 `bundled` 与 `judge-runtime` 两组；后者只把各归档中声明的成员复制到 `third_party/judge-runtime/<target>/`，并写入记录每个文件摘要的 `judge-runtime.json`。清单未声明的宿主（目前为没有官方构建的 macOS x64）只输出说明，不安装。`make sync` 通过 `install-locked-tools.mjs verify-judge-runtime` 按锁文件复核清单与全部摘要后复制到 `output/judge-runtime/`；已声明宿主缺失或文件被改动时同步失败并提示执行 `make deps tools`。发布布局校验要求已声明宿主的发行物包含该清单与两个库。CUDA 执行库因显卡与驱动差异不随包提供。

已验证归档和许可证按摘要缓存在 `third_party/build-input-cache/`。联网初始化填充缓存后，可以运行 pwsh -File scripts/install_bundled_tools.ps1 -Offline 和 pwsh -File scripts/install_protoc.ps1 -Offline 重建对应安装。缺失缓存或摘要损坏直接失败，不联网、不换版本。离线能力只覆盖这些输入；完整项目离线构建还需预先准备 Cargo、pnpm 等各自的依赖缓存。

安装使用独立暂存目录，先验证工具、许可证和 protoc 头文件，再替换目标。并发安装同一目标会明确拒绝；异常退出遗留的 .installing 目录需要确认旧安装进程已退出后清理，.previous 保存待恢复的旧安装，不可直接删除。常规替换失败会恢复旧目录。

发布构建每次执行前端与桌面的 pnpm install --frozen-lockfile，Rust 构建使用 --locked。依赖声明与锁文件不一致必须失败，不能在发行过程中更新锁文件。

## 类 Unix 完整发行目录切换

全部平台的分发 Rust 包与二进制名称以 `scripts/shipped-rust-packages.json` 为唯一清单。PowerShell 的构建及第三方声明入口、Unix 的 Cargo 包选择与发布校验共用该清单。Unix 必须把 Core、终端宿主、判定宿主与 TUI 的本轮成功制品全部带入暂存目录；缺失或重复制品直接失败，不能借用旧发行目录的 `bin` 文件补齐，且每种模式只写入自己的发行目录，另一模式目录中的文件必须保持不变。`node --test scripts/unix-distribution.test.mjs scripts/directory-publication.test.mjs` 在隔离目录验证 debug/release 完整发布、旧 TUI 掩盖缺失制品的拒绝，以及切换失败时保留旧发行物；该测试不替代真实 Rust、Electron 和应用运行验收。

Unix 构建在执行 Cargo 前，调用锁定 Electron npm 包自带的 `install.js` 核验运行时版本、路径标记及可执行文件。`node_modules` 存在不表示 Electron 运行时已下载；缺失时仍由官方安装器按该 npm 包内置校验和准备运行时，下载或解压失败立即终止，不跳过验证、不使用其他版本。网络受限时可以经其他机器取得同版本官方归档，完整摘要核验后填充该安装器的缓存，再由原安装器完成解压；不手写 `path.txt` 冒充安装成功。

先退出当前发行物的服务与桌面，再执行构建。Unix 构建入口在所有编译成功后，将已有同步资源复制到仓库 target 内的独立暂存目录，加入新二进制并运行既有桌面打包、布局校验。准备失败不改动正式目录；缺少同步资源仍需显式执行 make sync，不在构建时隐式同步。

切换先将正式目录改名为 .previous，再提升暂存目录。提升失败立即回滚；回滚本身失败时明确报告旧版保留路径。两个目录重命名之间存在短暂空窗，所以这是可恢复的版本切换，不承诺无中断的运行中升级或断电持久性。上一版保留到下次调用验证当前版本通过后才清理。

新进程可恢复中断的切换：正式目录缺失时恢复 .previous；新目录无效时将它移到独立 .rejected-* 目录供诊断，再恢复旧版。进程所有权保护拒绝并发发布；确认原进程已退出后可以回收锁。所有权文件缺失、损坏或进程身份仍存活时保守拒绝，需要操作者先核实，不能强行删除正在使用的目录。

目录级故障测试位于 scripts/directory-publication.test.mjs，覆盖准备失败、校验失败、磁盘不足、权限失败、回滚失败和重启恢复。它们不替代各平台真实 Electron、Chromium 与 Core 的打包及启动验收。

## 发行归档与一键安装

release 发行物必须自带受管 Python 与 Node。刚同步的发行目录从未运行过 Core，因此 release 打包会先执行随包服务的 `vulcan-code-service --prepare-managed-runtimes`（必须单独使用）。它以 Core 启动时相同的目录与锁调用同一安装器补齐缺失运行时，已就绪时不做任何事；失败即停止打包，不依赖先前运行过 Core。

发行目标共五个平台键：`windows-x64`、`macos-x64`、`macos-arm64`、`linux-x64`、`linux-arm64`，唯一定义是 `scripts/desktop-runtime.mjs` 的 `managedRuntimePlatformKey`（同时决定受管运行时与归档后缀）。正式发行由 `.github/workflows/release.yml` 在五个平台构建、签名并上传，流程见
[仓库质量门禁](repository-quality-gates.md)；本地需要发行物时在对应原生机器上执行 `make release`，不做交叉编译。

`node scripts/release-package.mjs package [--out <dir>]` 只打包、不编译：按启动时的标准校验当前 release 发行物（布局身份、全部入口及 release 资源），从随包服务 `--describe` 读取版本，生成 `target/release-packages/<版本>/vulcan-code-<版本>-<平台键>.tar.gz`（Windows 为 `.zip`，用系统自带 bsdtar 生成）。归档根即发行目录内容，不再包一层目录；同时重写 `SHA256SUMS`。Linux 还会打印随包二进制要求的 glibc 下限：归档继承构建机的 glibc，正式发行应在最旧的目标发行版上构建。各平台归档汇集到同一目录后，执行 `node scripts/release-package.mjs checksums <dir>` 重建索引；一个目录只允许一个版本，避免读取方面对同一平台的两个候选。上传到 GitHub Release 是人工步骤，把该目录全部文件作为资源上传即可。

`scripts/install.sh`（Linux 与 macOS）与 `scripts/install.ps1`（Windows PowerShell 5.1 与 PowerShell 7）是一键安装入口的模板，只在安装器发布（`release-setup.yml`）时由 `node scripts/entry-scripts.mjs generate` 填入安装器版本、下载源、各平台安装器摘要与 Windows 签名证书指纹，发布到 `setup/<版本>/`。脚本优先运行默认目录或 `--prefix` 对应的已安装程序，并为旧版安装器开启自动自更新。未找到时按 `uname`（Apple 芯片上的 Rosetta shell 仍取 arm64）或 Windows x64 选择安装器。`--route china` 与 `--route international` 分别将首次下载限定在中国大陆镜像与国际源。自动线路先试国际源，再试中国大陆镜像。下载后校验 SHA-256（Windows 另校验 Authenticode 签名者），再把原始参数交给安装器。用户取消时退出码为 130（`CANCELLED_EXIT_CODE`），`install.ps1` 据此安静结束。安装、更新与卸载都由安装器完成，见 [`app-updates.md`](app-updates.md)。产品发布不再附带安装脚本。

`node scripts/windows-installer.mjs package [--out <dir>] [--sign <命令>] [--setup <文件>]` 只打包、不编译，只支持在 Windows x64 上运行：用与 `release-package.mjs` 相同的校验检查当前 release 发行物并读取版本，然后用 Inno Setup 从同一模板生成未签名的完整安装程序 `target/release-packages/<版本>/VulcanCode-<版本>-windows-x64-setup.exe` 与更新包 `VulcanCode-<版本>-windows-x64-update.exe`，生成的 `.iss` 脚本保留在同一目录。更新包与安装程序的 AppId、快捷方式、注册表、卸载项与 `[Code]` 相同，但不含 `versions\<版本>`：它只用于跨安装纪元的应用内更新，在应用已组装好的版本上切换并刷新启动器与安装器，版本未组装完整时拒绝运行（见 [`app-updates.md`](app-updates.md)）。Inno Setup 的版本与安装器 SHA-256 锁定在 `scripts/build-inputs.lock.json` 的 `inno_setup`，首次运行时校验摘要后以便携模式静默安装到 `third_party/inno-setup/<版本>/`，不写入系统目录或卸载项。安装程序按当前用户安装到 `%LOCALAPPDATA%\Programs\VulcanCode`，不请求管理员权限，创建开始菜单快捷方式（桌面快捷方式可选），快捷方式指向桌面壳，由桌面壳按运行布局启动 Core。升级前与卸载时只清理发行物的顶层条目，安装目录中的其他文件和 `~/.vulcan` 下的用户数据保留。安装程序不进入 `SHA256SUMS`。`--setup` 给出已校验的 stable 安装器时，按原样装入安装目录的 `setup\`；本地构建不给出时安装程序不内置安装器。`node --test scripts/windows-installer.test.mjs` 验证生成脚本的命名、清理范围、权限级别与平台限制，不替代真实安装与启动验收。

`node --test scripts/release-package.test.mjs` 在全部平台验证归档命名、内容、链接与可执行位及索引，zip 格式只在 Windows 上验证。`node --test scripts/entry-scripts.test.mjs` 以合成安装器填写两个脚本并运行真实脚本：类 Unix 上用伪造的 `uname`/`sysctl` 验证 `install.sh` 的平台选择、下载源回退、摘要篡改与参数传递；Windows 上在本机已有的每种 PowerShell 中经本地 HTTP 服务验证 `install.ps1` 的参数与退出码传递、签名者与摘要校验。这些测试不替代各平台真实构建产物的安装与启动验收。
