# NoType

一个轻量的跨平台语音输入工具。按住快捷键说话，松开后文字直接打到光标位置。

和传统语音输入不同，NoType 不走 ASR 管线，而是把录音直接丢给多模态大模型（Gemini / Qwen-Omni），让模型原生理解语音内容后输出文字。好处是识别质量更高，能理解上下文语义，自动处理标点和格式。

## 功能

- 按住快捷键说话，松开后文字自动输入到当前光标位置
- 悬浮气泡跟随鼠标，实时显示录音/识别状态和转写结果
- 可编辑的提示词系统：角色指令 / 转录规则 / 专有词汇，在 app 里直接改
- 内置自动分段、数字格式化、符号口令、技术词汇校正等规则
- 系统托盘常驻，关闭窗口只是隐藏，不退出
- 快捷键可自定义（macOS 支持 Command 键）

## 支持的模型

| 厂商 | 模型 | 说明 |
|------|------|------|
| 阿里 | qwen3.5-omni-flash | 推荐日常使用，中文效果好，速度快 |
| 阿里 | qwen3.5-omni-plus | 质量最高，适合长段落和复杂场景 |
| Google | gemini-3-flash | Gemini 3 系列，多语言 |
| Google | gemini-3.1-flash-lite | 更便宜，适合高频使用 |

推荐优先使用 Qwen3.5-Omni 系列，中文场景下识别效果明显更好。

## 快速开始

1. 从 [Releases](../../releases) 下载安装
   - macOS：下载 `.dmg`，拖入 Applications
   - Windows：下载 `.msi`，双击安装
2. 启动后设置窗口自动弹出，填入 API Key
   - Qwen：去 [百炼控制台](https://dashscope.console.aliyun.com/) 申请
   - Gemini：去 [Google AI Studio](https://ai.google.dev) 申请
3. 按住快捷键说话（默认 `Ctrl+.`，macOS 建议改成 `Cmd+.`），松开后文字自动输入

应用常驻系统托盘，不用时不占前台。

## 提示词自定义

点击设置界面左上角的文档图标，可以编辑三个提示词模块：

- **角色指令**：定义转录引擎的行为，比如改成"将中文翻译成英文"就变成了翻译模式
- **转录规则**：控制标点、分段、数字格式化、符号口令等
- **专有词汇**：添加你领域的术语校正表，比如 `pie thon → Python`

修改后点 Save 即时生效，不用重启。

## 工作原理

```
按住快捷键 → 麦克风录音（16kHz WAV）→ base64 编码
  → 发给 Gemini/Qwen 多模态 API → 拿到文字
  → enigo 模拟键盘输入到当前窗口
```

## 配置

除了界面配置，也支持环境变量：

```bash
export NOTYPE_API_KEY="your-key"
export NOTYPE_PROVIDER="qwen"   # 或 gemini
export NOTYPE_MODEL="qwen3.5-omni-flash"
```

配置文件位置：
- macOS：`~/Library/Application Support/notype/config.toml`
- Windows：`%APPDATA%/notype/config.toml`

参考 [config.example.toml](config.example.toml)。

## 项目结构

```
src-tauri/          Tauri 主应用（托盘、快捷键、气泡窗口）
src/                前端设置界面 + 气泡 UI（TypeScript）
crates/
├── notype-audio/   录音（cpal）+ WAV 编码（hound）
├── notype-llm/     Gemini / Qwen API 客户端
├── notype-input/   键盘模拟（enigo）
└── notype-config/  配置管理 + 提示词系统
```

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
- [cpal](https://github.com/RustAudio/cpal) — 音频采集
- [enigo](https://github.com/enigo-rs/enigo) — 键盘模拟
- [reqwest](https://github.com/seanmonstar/reqwest) — HTTP 客户端

## License

MIT
感谢Linux.do社区
