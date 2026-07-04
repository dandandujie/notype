# NoType

**开源、自带 Key、低成本的 [Typeless](https://www.typeless.com) 替代品。** 按住快捷键说话，松开后经过 AI 整理的文字直接打到光标位置——去口水词、修改口、自动排版，读起来像认真打出来的。

对比 Typeless（$12–30/月、纯云端、免费层 8000 字/周）：

| | Typeless | NoType |
|---|---|---|
| 价格 | $12–30/月 | 开源免费，自带 API Key 按量付费 |
| 用量 | 免费层 8000 字/周 | 无限制 |
| 识别引擎 | 锁定官方云 | 6 种引擎任选，含完全本地的 Apple Speech / 本地 Whisper |
| 数据 | 云端处理 | Key 是你的，历史/统计全部本地 |
| AI 整理（去口水词/改口修正/自动排版） | ✅ | ✅ |
| 按应用调整语气 | ✅ | ✅（macOS） |
| 个人词典 | ✅ 自动+手动 | ✅ 自动（从你的纠错中学习）+手动 |
| 选中文本语音编辑 | ✅ | ✅（选中 + 按住编辑键说指令） |
| 逐字模式 / 边说边译 | ✅ | ✅（整理 / 逐字 / 译英 三档） |
| 统计与 streak | ✅ | ✅ 累计字数 / 语速 / 省时 / 连续天数 |
| 个性化进度 | ✅ | ✅「越用越懂你」进度条 |
| 首次引导 | ✅ | ✅ 四步：引擎 → 权限 → 试听写 |
| 声音反馈 | ✅ | ✅ 合成提示音（可关） |

识别链路（两类引擎，一套管线）：
- **多模态直识别**：Gemini / Qwen-Omni / Xiaomi MiMo，一次调用完成识别+润色
- **专用 ASR + LLM 润色**：火山引擎流式 ASR（官方 WebSocket，实时出字）/ OpenAI Whisper 兼容批量 ASR / Apple Speech（macOS 本地离线）/ Qwen3-ASR，粗转写交给任意 OpenAI 兼容 LLM 润色

## 功能

**输入**
- 按住快捷键说话松开输入（对讲机模式）；**快速短按锁定连续录音**，再按一下结束（解放手指）；主窗口点按录音键窗口内听写（Esc 取消）
- **自动发送**：可选输入完成自动回车——聊天软件说完即发
- **语音编辑**：在任意应用选中文本，按住编辑快捷键（默认 `Ctrl+,`）说指令——「改得正式一点」「翻译成英文」「压缩成三句话」——松开后选中文本被替换
- 两种输入方式：模拟键盘逐字输入 / 剪贴板粘贴（长文本更快）
- **流式逐字输出**：AI 边生成边打字，几乎无等待感；打字失败自动回退为一次性粘贴（设置可关）

**AI 整理（Typeless 同级）**
- 去口水词、去重复；说错改口时只保留最终意图；口述列表自动排版
- **确定性替换规则**：`含数 = 函数` 或 `/正则/ = 替换`，LLM 之外强制生效，专治顽固听错
- **应用场景感知**：识别你正在输入的应用——邮件更正式、微信更口语、Cursor/终端里保留英文术语（macOS）；支持自定义规则覆盖（`微信 = 语气亲切简短`）
- **三档输出风格**：智能整理 / 逐字转写 / 边说边译成英文，主页一键切换
- **结构化输出开关**：开 = 自动分段、列表排版；关 = 连续段落，适合聊天场景
- **润色服务自由选**：自定义任意 OpenAI 兼容厂商（DeepSeek / Moonshot / 本地 Ollama…）或 Qwen / Gemini / MiMo

**反馈与积累**
- 悬浮气泡 + 主窗口实时转写预览，波形由真实麦克风音量驱动；开始/完成/出错有细腻的合成提示音（可关）
- 转写历史：最近 200 条本地保存，按天分组，可搜索、复制、纠错、删除、清空，一键导出 Markdown 到下载目录
- **词典自动学习**：在结果卡片或历史里直接纠错，改动处（如「含数 → 函数」）自动学进词典，下次不再听错
- 统计仪表：累计字数、语速（字/分）、估算省时、连续使用天数
- **「越用越懂你」进度条**：词典积累、纠错学习、使用习惯的个性化程度可视化
- 个人词典：「听错的词 → 正确写法」快速添加，即时生效

**桌面集成**
- 首次启动四步引导：选引擎 → 授权限 → 试听写，三分钟上手
- 系统托盘常驻（含「开始/停止听写」入口），关闭窗口只是隐藏
- 麦克风设备可选、开机自启、辅助功能权限检测与引导
- 快捷键可自定义（macOS 支持 Command 键）

## 支持的识别引擎

| 引擎 | 类型 | 说明 |
|------|------|------|
| Qwen `qwen3.5-omni-flash / plus` | 多模态直识别 | 推荐日常使用，中文效果好；接入点可改成本地 vLLM/SGLang 部署 |
| Qwen `qwen3-asr-flash` | 专用 ASR | DashScope 专用识别模型，走 LLM 润色管线 |
| Gemini `gemini-3-flash / 3.1-flash-lite` | 多模态直识别 | 多语言，lite 适合高频 |
| Xiaomi MiMo `mimo-v2.5-asr` 等 | 多模态直识别 | 小米 MiMo 系列 |
| **火山引擎流式 ASR** | 专用 ASR（流式） | 官方豆包大模型识别 WebSocket 协议，说话时实时出字，纯 Rust 实现零外部依赖 |
| **Whisper 兼容** | 专用 ASR（批量） | 任意 `/v1/audio/transcriptions` 服务：OpenAI、Groq、whisper.cpp server、faster-whisper、本地 vLLM |
| **Apple Speech** | 专用 ASR（本地） | macOS 系统识别，免费、离线、零配置 |

专用 ASR 引擎的粗转写默认交给 LLM 润色（自动分段、数字格式化、符号口令、词汇校正、改口清理），润色服务可选任意 OpenAI 兼容厂商。

## 快速开始

1. 从 [Releases](../../releases) 下载安装
   - macOS：下载 `.dmg`，拖入 Applications
   - Windows：下载 `.msi`，双击安装
2. 启动后设置窗口自动弹出，选择识别引擎并填入凭证
   - Qwen：去 [百炼控制台](https://dashscope.console.aliyun.com/) 申请 Key；本地部署则把接入点改成本地地址
   - Gemini：去 [Google AI Studio](https://ai.google.dev) 申请
   - MiMo：去 [小米 MiMo API 开放平台](https://platform.xiaomimimo.com) 申请
   - 火山流式：去 [火山引擎语音控制台](https://console.volcengine.com/speech) 开通「豆包流式语音识别」，填 APP ID 和 Access Token
   - Whisper 兼容：填接入点（默认 OpenAI），本地 whisper.cpp / faster-whisper 服务可免 Key
   - Apple Speech：零配置，首次使用允许「语音识别」授权即可（macOS）
   - 使用专用 ASR 引擎时建议配置一个润色 LLM（AI 润色区，任意 OpenAI 兼容厂商均可）
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
按住快捷键 → 麦克风录音
  ├─ 多模态引擎（Gemini / Qwen-Omni / MiMo）：
  │    分片快照 → 实时预览；松开后单次调用完成识别+润色
  └─ 专用 ASR 引擎：
       火山流式：PCM 实时推流 → 逐句上屏 → 松开即收尾（最快）
       Whisper / Apple / Qwen-ASR：分片批量识别预览 → 松开后整段识别
       → 粗转写交给润色 LLM（任意 OpenAI 兼容）
  → LLM 输出流式逐字打进光标（失败自动一次性粘贴回退）
```

## 配置

除了界面配置，也支持环境变量：

```bash
export NOTYPE_API_KEY="your-key"
export NOTYPE_PROVIDER="qwen"   # gemini / mimo / volcengine / whisper / apple
export NOTYPE_MODEL="qwen3.5-omni-flash"

# 各引擎可选配置
export NOTYPE_QWEN_BASE_URL=""       # 本地部署 Qwen 时指向本地端点
export NOTYPE_MIMO_API_KEY=""
export NOTYPE_MIMO_BASE_URL="https://api.xiaomimimo.com/v1"
export NOTYPE_VOLC_APP_KEY=""
export NOTYPE_VOLC_ACCESS_KEY=""
export NOTYPE_VOLC_RESOURCE_ID="volc.bigasr.sauc.duration"
export NOTYPE_WHISPER_BASE_URL="https://api.openai.com/v1"
export NOTYPE_WHISPER_API_KEY=""
export NOTYPE_WHISPER_MODEL="whisper-1"

# LLM 润色
export NOTYPE_POSTPROCESS="true"
export NOTYPE_CUSTOM_LLM_BASE_URL=""   # 任意 OpenAI 兼容厂商
export NOTYPE_CUSTOM_LLM_API_KEY=""
export NOTYPE_CUSTOM_LLM_MODEL=""
```

配置文件位置：
- macOS：`~/Library/Application Support/notype/config.toml`
- Windows：`%APPDATA%/notype/config.toml`

转写历史保存在同目录的 `history.json`（最多 200 条，可在设置页一键打开该文件夹）。

参考 [config.example.toml](config.example.toml)。

## 项目结构

```
src-tauri/          Tauri 主应用（托盘、快捷键、气泡窗口）
src/                前端设置界面 + 气泡 UI（TypeScript）
crates/
├── notype-audio/   录音（cpal）+ WAV 编码（hound）
├── notype-llm/     识别客户端（Qwen / Gemini / MiMo / 火山流式 / Whisper / Apple）
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

macOS 上首次运行需要授权麦克风和辅助功能权限；使用 Apple Speech 引擎还需要「语音识别」权限。

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
