# Waku Mobile

原生 iOS / Android 客户端，通过 iroh P2P 连接你的 Waku 桌面主机，查看并继续已共享的 agent 会话。

## 功能

- **连接**：粘贴桌面端「设置 → P2P 远程」生成的票据，通过 iroh P2P 直连主机；票据保存在本机，下次启动自动静默重连
- **会话列表**：只显示主机上勾选了「允许远程同步与控制」的会话；状态点显示忙碌/空闲，标题、provider、模型、最近活跃时间
- **对话**：完整 transcript（用户/助手气泡、流式输出、权限卡片可一键 Respond）、发送/转向/停止、错误提示条
- **设置**：连接状态、断开连接并清除票据
- 所有计算（provider 进程、Git、文件系统）都在主机上运行——手机只是远程客户端

## 架构

```
mobile/                   独立 Cargo workspace（不与桌面端共享 gpui）
├── Cargo.toml            依赖 gpui-mobile + upstream zed gpui + waku-client
├── src/
│   ├── lib.rs            平台入口（Android android_main / iOS FFI）+ 开窗
│   ├── state.rs          WakuMobile 实体：连接、会话目录、聊天事件投影、轮询
│   ├── remote.rs         远程桥：连接、目录、hydrate、ensure_runtime
│   └── screens/          连接 / 会话 / 对话 / 设置 四个屏幕 + 导航 chrome
├── ios/                  Xcode 工程（xcodegen 生成）+ Objective-C 壳
├── android/              Gradle 工程（NativeActivity）+ Java glue
└── build.sh              一键构建脚本
```

### 为什么是独立 workspace

桌面端链接 `egoist/zed` 的 `waku-webview` 分支 gpui；gpui-mobile 构建在 **upstream zed gpui**（rev `5688167`）之上。Cargo 无法合并同一 semver 版本的不同 git 源，所以 `mobile/` 有自己的 Cargo.lock 和 `[patch.crates-io]`（async-task，与 gpui-mobile 示例一致）。根 workspace 的 `exclude = ["mobile"]` 防止误合并。

### 远程协议复用

手机完全复用桌面端的**远程客户端**角色：

- `DaemonSupervisor::connect_iroh(ticket)` — 拨号 + 握手
- `Command::LoadTaskState` — 共享会话目录（daemon 只返回 `remote_sync_enabled` 的会话）
- `Command::HydrateSession` — 打开会话时拉取完整 transcript
- `Command::AttachSession` / `ProbeProvider` + `Command::Start` — 附着现有 runtime，或为从未运行过的会话在主机上启动 provider
- `client.subscribe(session_id, runtime_id)` — 流式 driver 事件（TextDelta / TurnStarted / TurnFinished / Permission / …）
- `Command::Prompt` / `Command::Steer` / `Command::Cancel` / `Command::Respond` — 控制命令

### 状态投影

聊天视图的快照来自 `HydrateSession`，之后只由 driver 事件推进（与桌面端 `streaming.rs` 的模型一致）。`session_projection_precedes` 保护主机侧的权威 transcript，移动端从不回写会话状态。

## 构建

前置条件：

- **iOS**：Xcode 15+、[XcodeGen](https://github.com/yonaskolb/XcodeGen)（`brew install xcodegen`）、rustup target `aarch64-apple-ios` / `aarch64-apple-ios-sim`
- **Android**：Android SDK + NDK r25+、[cargo-ndk](https://github.com/nickelc/cargo-ndk)（`cargo install cargo-ndk`）、rustup target `aarch64-linux-android`

```bash
cd mobile

# iOS 模拟器（验证用）
./build.sh ios --simulator

# iOS 真机（需签名配置）
./build.sh ios --device

# Android 模拟器 / 真机
./build.sh android --emulator
./build.sh android --device

# 只编译不安装
./build.sh ios --simulator --no-run
```

手动检查编译（无需 Xcode / NDK）：

```bash
cargo check                                    # 宿主工具链
cargo check --target aarch64-apple-ios-sim     # iOS 模拟器
cargo ndk -t arm64-v8a -o android/gradle/app/src/main/jniLibs build   # Android
```

## 使用

1. 在桌面端 Waku 打开 **设置 → P2P 远程**，复制 iroh 票据
2. 手机打开 **Waku Mobile → 连接**，粘贴票据，点「连接远程主机」
3. 会话列表出现主机上共享的会话；点开即可对话
4. 在桌面端右键会话取消勾选「允许远程同步」，该会话立即从手机上消失

## 已知边界（v1）

- 文本输入是追加式（原生 IME 回调 + 全局缓冲），不支持光标移动/选区编辑
- 手机端不显示 reasoning/activities 细节，只有当前活动标题
- 主机上从未运行过、且 provider 未安装的会话无法启动
- 远程会话的 message 回退/checkpoint 在桌面端可用；手机上仅继续对话

## 已知问题（iOS 模拟器）

**iOS 模拟器上 app 会在启动数秒后崩溃**（SIGSEGV / SIGABRT），根因在 gpui-mobile 上游，不在 Waku 代码：

- gpui-mobile 的 iOS `Platform::run` 不阻塞（注释明确说明 "we don't need to start the run loop - UIApplicationMain handles that"），导致 `Application::run` 返回后 `Rc<AppCell>` 的强引用计数归零，整个 GPUI 状态（窗口、实体）被释放
- 之后 UIKit 的 `didBecomeActive` 等生命周期回调触发 GPUI 回调，`AsyncApp` 持有一个悬垂的 `Weak<AppCell>`，`upgrade()` 失败后 panic（`expect("app was released before async operation completed")`），在无法 unwind 的边界上变成 abort
- 这影响**任何**基于 gpui-mobile 的 iOS 模拟器 app，与 Waku 业务代码无关（gpui-mobile 示例 app 在模拟器上也无法完成链接）
- 已尝试的缓解（vendored fork）：`try_borrow_mut` 防重入、`IosWindow::drop` 从窗口列表注销、`catch_unwind` 包裹生命周期回调——这些阻止了部分 panic，但无法阻止 AppCell 释放后的核心崩溃，因为 Rust 的 `FnOnce` 闭包执行后必然释放其捕获的 `Rc`
- 正确修复需要在 gpui 侧保活 `AppCell`（例如在 `Application::run` 的闭包内 `std::mem::forget(this.clone())`），或让 iOS `Platform::run` 阻塞——都属于 gpui-mobile 上游改造

**验证建议**：在 iOS 真机上测试（真机启动时序不同，AppCell 释放前 UIKit 已完成 active 通知），或在 Android 上验证（Android 的 `Platform::run` 阻塞 event loop，`AppCell` 在进程生命周期内存活，无此问题）。
