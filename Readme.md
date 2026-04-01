# NoType

一个轻量的跨平台语音输入工具。按住快捷键说话，松开后文字直接打到光标位置。

和传统语音输入不同，NoType 不走 ASR 管线，而是把录音直接丢给多模态大模型（Gemini / Qwen-Omni），让模型原生理解语音内容后输出文字。好处是识别质量更高，能理解上下文语义，自动处理标点和格式。

## 支持的模型

| 厂商 | 模型 | 说明 |
|------|------|------|
| Google | gemini-3-flash | 速度快，日常首选 |
| Google | gemini-3.1-flash-lite | 更便宜，适合高频使用 |
| 阿里 | qwen3.5-omni-flash | 中文效果好 |
| 阿里 | qwen3.5-omni-plus | 质量最高 |
| 阿里 | qwen3.5-omni-light | 轻量版 |

## 快速开始

1. 从 [Releases](../../releases) 下载安装（macOS `.dmg` / Windows `.msi`）
2. 启动后自动弹出设置窗口，填入 API Key
   - Gemini：去 [ai.google.dev](https://ai.google.dev) 申请
   - Qwen：去 [dashscope.aliyuncs.com](https://dashscope.aliyuncs.com) 申请
3. 按住 `Ctrl+.` 说话，松开后文字自动输入

应用常驻系统托盘，不用时不占前台。

## 工作原理

```
按住 Ctrl+. → 麦克风录音（16kHz WAV）→ base64 编码
  → 发给 Gemini/Qwen 多模态 API → 拿到文字
  → 模拟键盘输入到当前窗口
```

没有中间商赚差价，音频直接交给大模型处理。

## 配置

除了界面配置，也支持环境变量（方便命令行用户）：

```bash
export NOTYPE_API_KEY="your-key"
export NOTYPE_PROVIDER="gemini"   # 或 qwen
export NOTYPE_MODEL="gemini-3-flash"
```

配置文件位置：
- macOS / Linux：`~/.config/notype/config.toml`
- Windows：`%APPDATA%/notype/config.toml`

参考 [config.example.toml](config.example.toml)。

## 项目结构

```
src-tauri/          Tauri 主应用（托盘、快捷键、调度）
src/                前端设置界面（TypeScript）
crates/
├── notype-audio/   录音（cpal）+ WAV 编码（hound）
├── notype-llm/     Gemini / Qwen API 客户端
├── notype-input/   键盘模拟（enigo）
└── notype-config/  TOML 配置 + 环境变量
```

Rust workspace 结构，各模块独立编译测试。

## 本地开发

需要 Rust 和 Node.js 20+。

```bash
npm install
npx tauri dev       # 开发模式运行
cargo test          # 跑测试
cargo clippy        # 代码检查
npx tauri build     # 构建安装包
```

macOS 上首次运行需要授权麦克风和辅助功能权限。

## 技术栈

- [Tauri v2](https://v2.tauri.app) — 跨平台桌面框架
- [cpal](https://github.com/RustAudio/cpal) — 跨平台音频采集
- [enigo](https://github.com/enigo-rs/enigo) — 跨平台键盘模拟
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP 客户端

## License

MIT
