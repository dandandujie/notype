# NoType

一个轻量的跨平台语音输入工具。按住快捷键说话，松开后文字直接打到光标位置。

现在支持两条识别链路：
- LLM 多模态识别（Gemini / Qwen）
- Doubao ASR（asr2api 网关模式 + 官方 API 模式）

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
| 豆包 | doubao-asr | 通过 asr2api 网关接入 |
| 豆包 | doubao-asr-official-flash | 官方极速版文件识别 |
| 豆包 | doubao-asr-official-standard | 官方标准版文件识别 |

推荐优先使用 Qwen3.5-Omni 系列，中文场景下识别效果明显更好。

## 快速开始

1. 从 [Releases](../../releases) 下载安装
   - macOS：下载 `.dmg`，拖入 Applications
   - Windows：下载 `.msi`，双击安装
2. 启动后设置窗口自动弹出，填入 API Key
   - Qwen：去 [百炼控制台](https://dashscope.console.aliyun.com/) 申请
   - Gemini：去 [Google AI Studio](https://ai.google.dev) 申请
   - Doubao asr2api：填写网关地址（默认 `http://127.0.0.1:8000`），可选网关 API Key
   - Doubao 官方：填写 `App Key` 和 `Access Key`
   - 使用 Doubao 时建议同时配置 Qwen 或 Gemini Key，用于第二阶段 LLM 实时后处理（未配置则退化为纯 ASR）
   - 若开启 Doubao 实时 WS 预览，需要本机 Python 环境安装 `doubaoime-asr`（见下文“Doubao WS 实时预览”）
   - 设置页已提供「一键安装依赖、初始化凭证并启动网关」按钮，会自动执行依赖安装、凭证初始化，并拉起本地 `127.0.0.1:8000` 网关服务
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
按住快捷键 → 麦克风实时分片录音（16kHz WAV）
  → ASR（Doubao / Gemini / Qwen）持续输出中间转写
  → （Doubao 模式）中间转写进入 LLM 实时后处理（语义修正/格式化/改口清理）
  → 松开按键后执行最终识别并输入到当前窗口
```

## 配置

除了界面配置，也支持环境变量：

```bash
export NOTYPE_API_KEY="your-key"
export NOTYPE_PROVIDER="qwen"   # 或 gemini / doubao
export NOTYPE_MODEL="qwen3.5-omni-flash"

# Doubao 可选配置
export NOTYPE_DOUBAO_BASE_URL="http://127.0.0.1:8000"
export NOTYPE_DOUBAO_API_KEY=""
export NOTYPE_DOUBAO_OFFICIAL_APP_KEY=""
export NOTYPE_DOUBAO_OFFICIAL_ACCESS_KEY=""
export NOTYPE_DOUBAO_POSTPROCESS="true"
export NOTYPE_DOUBAO_POSTPROCESS_PROVIDER="auto" # auto / qwen / gemini
export NOTYPE_DOUBAO_REALTIME_WS="true"
export NOTYPE_DOUBAO_IME_CREDENTIAL_PATH="~/.config/doubaoime-asr/credentials.json"
# 可选：指定 Python 解释器（默认 python3）
export NOTYPE_DOUBAO_PYTHON="python3"
```

配置文件位置：
- macOS：`~/Library/Application Support/notype/config.toml`
- Windows：`%APPDATA%/notype/config.toml`

参考 [config.example.toml](config.example.toml)。

## Doubao WS 实时预览

`provider=doubao` 且模型为 `doubao-asr`（asr2api 模式）时，可启用 WS 实时预览链路：

`麦克风 PCM 分片 -> doubaoime-asr realtime -> 气泡实时粗转写 -> LLM 实时后处理`

依赖安装示例：

```bash
pip install doubaoime-asr
```

说明：

- `DOUBAO_IME_CREDENTIAL_PATH`（NoType 内部会设置为配置项 `doubao_ime_credential_path`）指向 `doubaoime-asr` 的凭证文件。
- 一键配置后会自动启动本地 asr2api 网关，并在应用运行期间定时巡检，异常时自动尝试拉起。
- WS 桥接不可用、握手超时或中途异常时，会自动回退到当前已有的分片 ASR 实时预览，不影响按键松开后的最终识别流程。

## 项目结构

```
src-tauri/          Tauri 主应用（托盘、快捷键、气泡窗口）
src/                前端设置界面 + 气泡 UI（TypeScript）
crates/
├── notype-audio/   录音（cpal）+ WAV 编码（hound）
├── notype-llm/     Gemini / Qwen / Doubao 识别客户端
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
----
感谢Linux.do社区
https://linux.do
