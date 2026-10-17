<p align="center">
  <img src="./docs/rivium.png" alt="Rivium 标志" width="60%" />
</p>

<h1 align="center">Rivium</h1>

<p align="center">
  <strong>面向长期运行应用的 Rust 服务基础设施</strong><br />
  统一独立进程与 Android JNI 宿主的生命周期。
</p>

<p align="center">
  <a href="README.md">English</a> | <strong>简体中文</strong>
</p>

<p align="center">
  <a href="https://crates.io/crates/rivium"><img src="https://img.shields.io/crates/v/rivium.svg" alt="Crates.io" /></a>
  <a href="https://docs.rs/rivium"><img src="https://docs.rs/rivium/badge.svg" alt="文档" /></a>
  <a href="https://github.com/rivium-rs/rivium/actions/workflows/ci.yml"><img src="https://github.com/rivium-rs/rivium/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/MSRV-1.92%2B-orange.svg" alt="最低支持 Rust 版本 1.92+" />
  <img src="https://img.shields.io/crates/l/rivium.svg" alt="许可证" />
</p>

<p align="center">
  <a href="https://docs.rs/rivium">已发布版本 API 文档</a> |
  <a href="docs/architecture.md">架构</a> |
  <a href="docs/platforms.md">平台支持</a> |
  <a href="docs/contributing/README.md">参与贡献</a>
</p>

## 概览

Rivium 处理服务中常见的基础设施工作，让应用专注于自身业务：

- **一套生命周期，两种宿主。** 同一个应用可以作为独立进程运行，也可以使用嵌入式宿主，
  共享启动、就绪、优雅关停和重启机制。

- **配置与日志。** 支持带来源追踪的分层配置、经过校验的配置重载，以及控制台、
  限制磁盘占用的文件日志和 Android logcat。应用显式选择允许运行时变更的配置项。

- **明确的资源归属。** 任务作用域、服务监管和宿主报告区分停止请求与实际完成的清理。
  任务、HTTP 并发和文件日志队列均有容量限制；重启前会等待上一运行代释放资源。

- **按需接入。** 通过独立 crate 添加 axum HTTP 集成、JNI 桥接或测试辅助工具。
  不使用这些集成的服务，无需引入对应依赖。

Rivium 基于 Tokio，提供服务基础设施，不替代异步运行时。
生命周期和 crate 职责划分详见[架构指南](docs/architecture.md)。

## 快速开始

**0.2.0 工作区尚未发布。** 使用 Rust 1.92 或更新版本，从当前仓库运行基础服务：

```sh
cargo run -p rivium-examples --example basic-service -- --root target/basic-example
```

服务就绪后会等待 Ctrl-C，再执行关停。若要在独立应用中使用当前源码，添加核心 crate 和进程宿主。
下方示例中的 `../rivium` 是从应用目录到本仓库的相对路径，请按实际目录布局调整：

```toml
[dependencies]
rivium = { path = "../rivium/crates/rivium", features = ["rt"] }
rivium-host = { path = "../rivium/crates/rivium-host", features = ["process"] }
serde = { version = "1", features = ["derive"] }
```

实现 `rivium_host::App`，定义配置类型和服务列表，再从 `main` 调用
`rivium_host::process::run::<YourApp>()`。服务在初始化完成后声明就绪，
收到停止信号后完成收尾，并在返回前释放持有的资源。

[宿主指南](docs/architecture.md#hosts)介绍应用模型；
[基础服务示例](examples/basic-service.rs)提供了完整实现。

同一个应用也可以通过 `rivium_jni::export!` 导出为 JNI 库。

按需添加集成，并让所有 Rivium crate 使用同一份仓库源码：

```toml
[dependencies]
rivium-http = { path = "../rivium/crates/rivium-http", features = ["server"] } # HTTP API
rivium-jni = { path = "../rivium/crates/rivium-jni" } # Android JNI 库

[dev-dependencies]
rivium-test = { path = "../rivium/crates/rivium-test" }
```

可运行的服务示例和 JNI 集成方式见[示例指南](examples/README.md)。

本 README 对应 **0.2 工作区**，其 Rust API 和 crate 划分与 0.1 不同。
在 1.0 之前，新的次版本可能包含 API 破坏性变更。
已发布版本见[发布说明](https://github.com/rivium-rs/rivium/releases)，迁移步骤见
[升级指南](docs/upgrading.md)。

页面上方的徽章和 API 链接指向已发布版本；
当前仓库中的示例和指南则与本工作区源码对应。

## Crates

| Crate | 职责 |
| --- | --- |
| [`rivium`](crates/rivium/README.md) | 结构化错误；`task` 提供任务作用域；`rt` 提供服务监管、就绪状态和关停报告 |
| [`rivium-util`](crates/rivium-util/README.md) | 可独立启用的配置、日志、日志导出和持久任务对账，以及原子文件替换 |
| [`rivium-host`](crates/rivium-host/README.md) | 应用组装、配置重载、运行时管理，以及进程和嵌入式宿主 |
| [`rivium-http`](crates/rivium-http/README.md) | HTTP 响应类型，以及可选的有界请求处理约定、受监管监听器和文件下载能力 |
| [`rivium-jni`](crates/rivium-jni/README.md) | 嵌入式宿主的 JNI 桥接，自动生成原生入口 |
| [`rivium-test`](crates/rivium-test/README.md) | 临时目录、日志捕获和生命周期辅助工具；仅作为开发依赖使用 |

核心、工具、宿主和 HTTP crate 默认不启用任何 feature。核心错误 API 不依赖外部库；
宿主始终包含配置、日志和运行时依赖，HTTP 始终依赖 axum。具体能力的启用方式见各 crate 的文档。

## 支持的 Rust 版本

最低支持 Rust 版本（MSRV）为 **1.92**，由独立的 CI 检查验证。

## 平台支持

Rivium 面向 GNU/Linux、macOS 和 Windows 进程，以及通过 JNI 接入的 Android 应用。

验证范围随目标平台和版本而变化：原生测试、模拟器测试和设备测试不能相互替代。
当前 Android 检查覆盖编译和 JNI ELF 属性，不包含设备上的实际运行。
历史平台验证结果不能直接沿用到本次重写；0.2 候选版本的 scope 和任务管理性能退化已得到改善，
性能验收仍待完成。

当前验证范围、已知限制和[各版本验证记录](docs/platforms.md#release-records)
详见[平台支持](docs/platforms.md)。

## 获取帮助

可以先查阅[已发布版本 API 文档](https://docs.rs/rivium)和[架构指南](docs/architecture.md)。

如果有不清楚或无法正常工作的地方，请[提交 issue](https://github.com/rivium-rs/rivium/issues)，
附上 Rivium 版本、目标平台和一个最小复现示例。也欢迎提出建议或改进文档。

## 开发

提交与评审流程见[贡献指南](docs/contributing/README.md)。本地环境配置、原生测试、
JVM 契约和跨目标验证见[开发检查](docs/development.md)。

维护者可在[发布指南](docs/releasing.md)中查阅预演和发布步骤。

## 许可证

你可以选择按 [Apache License, Version 2.0](LICENSE-APACHE) 或
[MIT 许可证](LICENSE-MIT)使用本项目。

除非你明确另作声明，任何有意提交并纳入本项目的贡献
（按 Apache-2.0 许可证中的定义），均按上述双许可证条款授权，不附加其他条款或条件。
