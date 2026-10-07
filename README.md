# stemscan

**判断一首歌是「纯器乐 (BGM)」还是「人�?+ BGM」—�?不猜，把人声真的拆出来量�?*

给口播视频配 BGM 时，你需要的�?*没有人声**的曲子；把带人声的歌铺在解说下面，人声会打架�?stemscan 用成熟的音源分离模型把音轨拆成人�?伴奏，再�?Rust 里做能量统计给出判定�?
一�?struct 派生 **CLI / TUI / Web / MCP** 四端（[lilyco](https://github.com/lilyco-42/lilyco)），天生 AI-callable�?
---

## 为什么不用现成的「人声检测�?
开工前调研�?GitHub�?026-10）：

| 方向 | 代表项目 | 结论 |
|---|---|---|
| vocal detection | `georgid/vocal-detection`、`NTUT-LabASPL/VocalDetection` | 基本�?*论文数据集与标注**，不是能用的工具 |
| 音源分离 | `Anjok07/ultimatevocalremovergui` �?6k、`facebookresearch/demucs` �?0k、`nomadkaraoke/python-audio-separator` �?.4k | �?成熟、MIT、活�?|
| 音乐标注 | `musicnn` �?| �?TF，标签粒度粗，装起来�?|

�?�?*分离 + 自己算能�?*：依赖最少、结果可解释、阈值可调。分离本身不重造轮子�?
## 原理

1. `ffmpeg` 解码�?44.1k 立体�?2. `audio-separator`（UVR 生态）拆出 **Vocals** �?**Instrumental** 两轨
3. Rust 侧对两轨�?100ms 分帧统计�?   - `vocal_share_pct` �?人声能量 /（人�?+ 伴奏）能�?   - `active_frame_ratio_pct` �?「人声活跃帧」占比（该帧人声高于峰�?�?4dB **�?*压过伴奏 0.18 倍）
4. 双阈值判定，任一命中即判有人声；离阈值越�?`confidence` 越高

## 实测数据

9 首歌（含�?�?英文，流行、器�?OST、type beat），�?**faster-whisper 转写分离出的人声�?*做独立交叉验证：

| 曲目 | 人声能量% | 活跃�? | whisper 转写人声�?| stemscan |
|---|---|---|---|---|
| Bôa - Duvet | 21.1 | 80.5 | 313 字英文歌�?| vocal �?|
| SpendyMily - Iris | 42.7 | 80.8 | 178 字日文歌�?| vocal �?|
| Terror Jr - 3 Strikes | 27.4 | 81.1 | 343 字英文歌�?| vocal �?|
| 茉ひ�?RINZO - フレグランス | 42.5 | 72.8 | 98 字日文歌�?| vocal �?|
| Glass Animals - The Other Side Of Paradise | 5.5 | 48.1 | 194 字英文歌�?| vocal �?|
| KID - 回到以后 | 5.4 | 39.4 | 有歌�?| vocal �?|
| MRZ - rainy night | 0.1 | 1.8 | 空（幻觉�?| instrumental �?|
| 牛尾憲輔 - reflexion,allegretto,you | 0.4 | 0.5 | �?| instrumental �?|
| 上海アリス幻樂団 - 衛星カフェテラス | 0.0 | 0.0 | �?| instrumental �?|

**9/9 与独立验证一致�?*

两个簇之间有**很宽的空�?*（器�?�?.4% / �?.8%，人�?�?.4% / �?9.4%），
所以阈值放在空档中间即可，不需要精调�?
> 注意 5.4% / 48.1% 那两首：人声能量占比看着低，是因为编曲把人声压得比伴奏低 12�?6 dB�?> �?*活跃帧占�?*照样很高。这也是为什么用双指标而不是单看能量�?
## 安装

```sh
cargo install --git https://github.com/lilyco-42/stemscan
```

还要一个分离后端（一次性）�?
```sh
pip install audio-separator audioread
```

> `audioread` 不是 `audio-separator` 的声明依赖，�?0.47.0 少了它会
> `ModuleNotFoundError`。直接装上省事�?
## 用法

```sh
# 扫一个目录（自动跳过 .ncm 这类非音频）
stemscan --input "C:/CloudMusic/VipSongsDownload" --seconds 60 --out report.json

# 单文件，看人话输�?stemscan --input song.mp3

# �?Agent �?stemscan --input <dir> --json-stream     # 进度�?stemscan --mcp                            # MCP 服务�?stemscan --schema                         # JSON Schema
```

| 参数 | 默认 | 说明 |
|---|---|---|
| `--seconds` | 60 | 只分析前 N 秒（0 = 全曲）。分类不需要听完整�?|
| `--model` | `1_HP-UVR.pth` | 分离模型�?*VR 系列�?CPU 上最�?*（≈1x 实时）；MDX 更准但慢 5�?0 �?|
| `--threshold` | 2 | 人声能量占比阈�?% |
| `--active-threshold` | 15 | 人声活跃帧占比阈�?% |
| `--keep-stems` | off | 保留分离出的音轨，便于交叉验�?|

判定结果�?
| verdict | label |
|---|---|
| `vocal` | 人声 + BGM |
| `instrumental` | 纯器乐（可作 BGM�?|

## 性能（Windows / CPU only，实测）

- VR 模型�?0s 音频 �?39s 分离，加 ~17s 进程启动（模型加载）
- **每个文件都要付一�?~17s 模型加载**（CLI 是独立进程）
- 9 首歌 × 60s 窗口 �?9 分钟

> 想更快：把所有文件塞�?*一�?* `audio-separator` 调用（它接受多个输入路径），
> 模型只加载一次。代价是失去逐文件进度�?
## 依赖

- `ffmpeg` / `ffprobe` �?PATH（或 `--ffmpeg` 指定�?- `audio-separator` �?PATH、`$AUDIO_SEPARATOR`、或 `--separator` 指定
- 首次运行会自动下载分离模�?
## 踩过的坑

- **`python -m audio_separator` 不存�?*（没�?`__main__`），要用 `audio-separator` 可执行文�?- `audio-separator` 0.47.0 �?`audioread` 依赖
- `--list_models` 不加 `--list_format json` 会崩：`max() iterable argument is empty`
- **默认模型�?BS-Roformer**：CPU �?40s 音频要跑 **10 分钟**，换�?VR 系列�?15 �?- lilyco �?`AppError::Io` 收的�?`std::io::Error`（带 `#[from]`），
  字符串错误要�?`AppError::Runtime`，否�?`expected Error, found String`
- `ffmpeg -v error` 会吞�?`volumedetect` 的输出（它在 INFO 级），量音量时别�?
## 许可�?
MIT
