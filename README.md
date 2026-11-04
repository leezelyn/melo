# 环间音乐传歌 · melo

AstroBox API Level 3 原生传歌插件，当前源码版本 **0.2.7**。使用 Rust 编译为 WASI Preview 2 / WebAssembly Component。直接使用 ABox 已连接的第一台设备，无需额外连接或选择设备。

## 功能

- 选择 MP3，编辑歌曲名称与起止时间，最长 300 秒、文件最大 64 MiB。
- 原版、轻度增强、明显增强三档音质，切换后重新转换；可试听转换结果。
- 添加 UTF-8 LRC，随片段裁剪并同步时间。
- 稳定、均衡、快速三档传输速度；固定 8 px 的细进度条。
- 分块 ACK、CRC32 校验、有界重试、取消确认和最终保存回执；仅在确认保存后显示 100%。
- 每次新建任务，不持久化或接管旧任务编号。手环有旧任务时提示在手环“传歌 → 取消旧任务”后重试。

音频保持 12 kHz 单声道 s8，三档不会改变时长和文件大小，区别在响度与动态处理：

| 音质 | 峰值 | 压缩阈值 / 比率 | 起效 / 释放 | 最大增益补偿 |
| --- | --- | --- | --- | --- |
| 原版 | 64 | 不压缩 | — | 6 dB |
| 轻度增强 | 64 | -12 dB / 2:1 | 5 / 120 ms | 6 dB |
| 明显增强 | 127 | -24 dB / 4:1 | 1 / 40 ms | 18 dB |

这些参数沿用环间音乐安卓端已有预设；原生马达增益不变。试听、实际发送与回执核验使用同一份音频及峰值参数。

## 构建

需要 Node.js 22+、npm 和支持 Edition 2024 的 Rust stable。

```sh
git clone https://github.com/leezelyn/melo.git
cd melo
rustup target add wasm32-wasip2
npm ci
npm test
npm run package
```

`npm run build` 生成 `dist/plugin.wasm` 和 manifest；`npm run package` 额外执行 ABI 与实际 WASM 回调检查，生成 `dist/HuanjianMusic-AstroBox-0.2.7.abp`。插件源所需的 `dist/manifest.json`、`dist/plugin.wasm`、`dist/icon.png` 随发布提交，其他构建产物及依赖缓存忽略。

## 插件源

根目录 `index.txt` 指向 `dist`。本插件收录于 [leezelyn 的插件源](https://github.com/leezelyn/AstroBox-NG-Plugin-Repo)，该源的 `index.txt` 使用：

```text
https://raw.githubusercontent.com/leezelyn/melo/refs/heads/main/
```

发布更新时先执行 `npm run package`，再一并提交源码 manifest 与上述三份发布文件。

## 代码位置

| 路径 | 内容 |
| --- | --- |
| `native/plugin.rs` | 原生界面、文件选择、宿主调用与事件 |
| `native/audio.rs` | MP3 解码、重采样、三档动态处理与试听 |
| `native/transfer.rs` | 分块传输状态机、重试、取消与回执 |
| `native/incoming.rs` | ABox 包裹回包及旧格式适配 |
| `native/lyrics.rs` | LRC 校验及裁剪 |
| `wit/` | API3 接口定义及其原有许可证 |
| `tests/` | 测试音频、接收器快照和协议集成测试 |
| `tools/runtime38/` | 与 V3 宿主逐次回调方式匹配的 Wasmtime 38.0.4 运行器 |

这是独立插件仓库。接收器测试所需 JS 已冻结在 `tests/fixtures/receivers` 并记录来源及 SHA-256，不依赖 music_player 仓库的相邻目录。这里不包含可安装的手环应用、Android 应用或旧 API4 实现。

## 接入说明

manifest 使用 `api_level=3`、`wasi_version=2`，声明 `device`、`thirdpartyapp`、`interconnect` 与独立的 `register_interconnect_recv` 权限。发送时先注册回执，再自动打开 `com.tse.musicplayer` 的 `pages/index`。无需手环手动确认接收。

手环需已安装兼容的环间音乐接收器。10 Pro 0.2.43 新增本地“取消旧任务”；已保存歌曲会保留。只有旧版接收器且已遗留持久化任务时，插件会停止并提示，不能代替手环本地清理未知任务。

请停止手环播放，并保持播放器页面打开。官方 V3 宿主对单次回调有约 30 秒上限，文件选择应在此期间完成或取消，超时可能需要重新启用插件。

## 验证范围

`npm test` 包含 24 项 Rust 测试及 12 项基于真实接收器代码快照的 JS 集成测试。打包检查使用实际 WASM，覆盖文件选择、转换、三档切换后的缓存失效、自动目标、定时分块、保存回执与旧任务阻塞。

这些检查使用本地文件、宿主和通信适配器，没有连接真实设备。最终 ABox 导入、界面、试听及手环播放由用户验收。
