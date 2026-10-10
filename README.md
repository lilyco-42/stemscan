# stemscan

**判断一首歌是「纯器乐 (BGM)」还是「人声 + BGM」—— 不猜，把人声真的拆出来量。**

给口播视频配 BGM 时，你需要的是**没有人声**的曲子；把带人声的歌铺在解说下面，人声会打架。
stemscan 用成熟的音源分离模型把音轨拆成人声/伴奏，再在 Rust 里做能量统计给出判定。

一个 struct 派生 **CLI / TUI / Web / MCP** 四端（[lilyco](https://github.com/lilyco-42/lilyco)），天生 AI-callable。

---

## 为什么不用现成的「人声检测」

开工前调研过 GitHub（2026-10）：

| 方向 | 代表项目 | 结论 |
|---|---|---|
| vocal detection | `georgid/vocal-detection`、`NTUT-LabASPL/VocalDetection` | 基本是**论文数据集与标注**，不是能用的工具 |
| 音源分离 | `Anjok07/ultimatevocalremovergui` ★26k、`facebookresearch/demucs` ★10k、`nomadkaraoke/python-audio-separator` ★1.4k | ✅ 成熟、MIT、活跃 |
| 音乐标注 | `musicnn` 等 | 要 TF，标签粒度粗，装起来重 |

→ 走**分离 + 自己算能量**：依赖最少、结果可解释、阈值可调。分离本身不重造轮子。

## 原理

1. `ffmpeg` 解码成 44.1k 立体声
2. `audio-separator`（UVR 生态）拆出 **Vocals** 与 **Instrumental** 两轨
3. Rust 侧对两轨做 100ms 分帧统计：
   - `vocal_share_pct` — 人声能量 /（人声 + 伴奏）能量
   - `active_frame_ratio_pct` — 「人声活跃帧」占比（该帧人声高于峰值 −34dB **且**压过伴奏 0.18 倍）
4. 双阈值判定，任一命中即判有人声；离阈值越远 `confidence` 越高

## 实测数据

9 首歌（含中/日/英文，流行、器乐 OST、type beat），用 **faster-whisper 转写分离出的人声轨**做独立交叉验证：

| 曲目 | 人声能量% | 活跃帧% | whisper 转写人声轨 | stemscan |
|---|---|---|---|---|
| Bôa - Duvet | 21.1 | 80.5 | 313 字英文歌词 | vocal ✓ |
| SpendyMily - Iris | 42.7 | 80.8 | 178 字日文歌词 | vocal ✓ |
| Terror Jr - 3 Strikes | 27.4 | 81.1 | 343 字英文歌词 | vocal ✓ |
| 茉ひる,RINZO - フレグランス | 42.5 | 72.8 | 98 字日文歌词 | vocal ✓ |
| Glass Animals - The Other Side Of Paradise | 5.5 | 48.1 | 194 字英文歌词 | vocal ✓ |
| KID - 回到以后 | 5.4 | 39.4 | 有歌词 | vocal ✓ |
| MRZ - rainy night | 0.1 | 1.8 | 空（幻觉） | instrumental ✓ |
| 牛尾憲輔 - reflexion,allegretto,you | 0.4 | 0.5 | 空 | instrumental ✓ |
| 上海アリス幻樂団 - 衛星カフェテラス | 0.0 | 0.0 | 空 | instrumental ✓ |

**9/9 与独立验证一致。**

两个簇之间有**很宽的空档**（器乐 ≤0.4% / ≤1.8%，人声 ≥5.4% / ≥39.4%），
所以阈值放在空档中间即可，不需要精调。

> 注意 5.4% / 48.1% 那两首：人声能量占比看着低，是因为编曲把人声压得比伴奏低 12–16 dB，
> 但**活跃帧占比**照样很高。这也是为什么用双指标而不是单看能量。

## 安装

```sh
cargo install --git https://github.com/lilyco-42/stemscan
```

还要一个分离后端（一次性）：

```sh
pip install audio-separator audioread
```

> `audioread` 不是 `audio-separator` 的声明依赖，但 0.47.0 少了它会
> `ModuleNotFoundError`。直接装上省事。

## 用法

```sh
# 扫一个目录（自动跳过 .ncm 这类非音频）
stemscan --input "C:/CloudMusic/VipSongsDownload" --seconds 60 --out report.json

# 单文件，看人话输出
stemscan --input song.mp3

# 给 Agent 用
stemscan --input <dir> --json-stream     # 进度流
stemscan --mcp                            # MCP 服务器
stemscan --schema                         # JSON Schema
```

| 参数 | 默认 | 说明 |
|---|---|---|
| `--seconds` | 60 | 只分析前 N 秒（0 = 全曲）。分类不需要听完整首 |
| `--model` | `1_HP-UVR.pth` | 分离模型。**VR 系列在 CPU 上最快**（≈1x 实时）；MDX 更准但慢 5–10 倍 |
| `--threshold` | 2 | 人声能量占比阈值 % |
| `--active-threshold` | 15 | 人声活跃帧占比阈值 % |
| `--keep-stems` | off | 保留分离出的音轨，便于交叉验证 |

判定结果：

| verdict | label |
|---|---|
| `vocal` | 人声 + BGM |
| `instrumental` | 纯器乐（可作 BGM） |

## 性能（Windows / CPU only，实测）

- VR 模型：40s 音频 ≈ 39s 分离，加 ~17s 进程启动（模型加载）
- **每个文件都要付一次 ~17s 模型加载**（CLI 是独立进程）
- 9 首歌 × 60s 窗口 ≈ 9 分钟

> 想更快：把所有文件塞进**一次** `audio-separator` 调用（它接受多个输入路径），
> 模型只加载一次。代价是失去逐文件进度。

## 依赖

- `ffmpeg` / `ffprobe` 在 PATH（或 `--ffmpeg` 指定）
- `audio-separator` 在 PATH、`$AUDIO_SEPARATOR`、或 `--separator` 指定
- 首次运行会自动下载分离模型

## 踩过的坑

- **`python -m audio_separator` 不存在**（没有 `__main__`），要用 `audio-separator` 可执行文件
- `audio-separator` 0.47.0 缺 `audioread` 依赖
- `--list_models` 不加 `--list_format json` 会崩：`max() iterable argument is empty`
- **默认模型是 BS-Roformer**：CPU 上 40s 音频要跑 **10 分钟**，换成 VR 系列快 15 倍
- lilyco 的 `AppError::Io` 收的是 `std::io::Error`（带 `#[from]`），
  字符串错误要用 `AppError::Runtime`，否则 `expected Error, found String`
- `ffmpeg -v error` 会吞掉 `volumedetect` 的输出（它在 INFO 级），量音量时别加

## 许可证

MIT
