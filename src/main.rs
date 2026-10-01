//! stemscan — 判断一首歌是「纯器乐 (BGM)」还是「人声 + BGM」。
//!
//! 做法：先用成熟的音源分离模型（audio-separator / UVR-MDX）把人声和伴奏拆开，
//! 再用本程序在 Rust 里做信号统计（人声能量占比 + 人声活跃帧占比），最后判定。
//!
//! 一个 struct，四种界面：CLI / TUI / Web / MCP。

use lilyco::prelude::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const AUDIO_EXT: &[&str] = &[
    "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aiff", "aif",
];

#[derive(App)]
#[app(
    about = "判断音频是纯器乐(BGM)还是人声+伴奏 —— 音源分离 + 能量统计",
    run = "run"
)]
struct StemScan {
    #[arg(about = "音频文件或目录", must_exist = true)]
    input: PathBuf,

    #[arg(about = "递归扫描子目录")]
    recursive: bool,

    #[arg(about = "只分析前 N 秒（0 = 全曲）", default = 60, range = 0..=7200)]
    seconds: u32,

    #[arg(about = "人声能量占比阈值 %，超过即判定有人声", default = 2, range = 1..=100)]
    threshold: u8,

    #[arg(about = "人声活跃帧占比阈值 %，超过即判定有人声", default = 15, range = 1..=100)]
    active_threshold: u8,

    #[arg(
        about = "分离模型文件名（VR 模型在 CPU 上最快，默认 1_HP-UVR）",
        default = "1_HP-UVR.pth"
    )]
    model: String,

    #[arg(about = "audio-separator 可执行文件（留空自动查找）")]
    separator: Option<String>,

    #[arg(about = "ffmpeg 可执行文件", default = "ffmpeg")]
    ffmpeg: String,

    #[arg(about = "保留分离出的 stems")]
    keep_stems: bool,

    #[arg(about = "报告 JSON 输出路径")]
    out: Option<PathBuf>,
}

// ---------------------------------------------------------------- helpers

fn collect_inputs(input: &Path, recursive: bool) -> Result<Vec<PathBuf>, AppError> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    if !input.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "不是文件也不是目录: {}",
            input.display()
        )));
    }
    let mut out = Vec::new();
    let mut stack = vec![input.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir)
            .map_err(|e| AppError::Runtime(format!("读取目录 {} 失败: {e}", dir.display())))?;
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if recursive {
                    stack.push(p);
                }
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str())
                && AUDIO_EXT.contains(&ext.to_ascii_lowercase().as_str())
            {
                out.push(p);
            }
        }
    }
    out.sort();
    if out.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "{} 下没找到音频文件",
            input.display()
        )));
    }
    Ok(out)
}

fn find_separator(explicit: &Option<String>) -> Result<String, AppError> {
    if let Some(s) = explicit {
        return Ok(s.clone());
    }
    if let Ok(s) = std::env::var("AUDIO_SEPARATOR")
        && !s.is_empty()
    {
        return Ok(s);
    }
    // 1) on PATH
    if Command::new("audio-separator")
        .arg("--help")
        .output()
        .is_ok()
    {
        return Ok("audio-separator".into());
    }
    // 2) common managed-venv locations
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let cands = [
        format!(
            "{home}\\.workbuddy-ai\\binaries\\python\\envs\\default\\Scripts\\audio-separator.exe"
        ),
        format!("{home}/.local/bin/audio-separator"),
        "/usr/local/bin/audio-separator".to_string(),
    ];
    for c in cands {
        if Path::new(&c).exists() {
            return Ok(c);
        }
    }
    Err(AppError::Runtime(
        "找不到 audio-separator。请 pip install audio-separator，或用 --separator 指定路径".into(),
    ))
}

fn run_cmd(prog: &str, args: &[String]) -> Result<String, AppError> {
    let out = Command::new(prog)
        .args(args)
        .output()
        .map_err(|e| AppError::Runtime(format!("执行 {prog} 失败: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Runtime(format!(
            "{prog} 退出码 {:?}: {}",
            out.status.code(),
            err.lines().rev().take(6).collect::<Vec<_>>().join(" | ")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn probe_duration(ffmpeg: &str, file: &Path) -> f64 {
    let ffprobe = ffmpeg.replace("ffmpeg", "ffprobe");
    let out = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(file)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(0.0),
        Err(_) => 0.0,
    }
}

// ---------------------------------------------------------------- analysis

struct Metrics {
    dur: f64,
    vocal_rms_db: f64,
    inst_rms_db: f64,
    vocal_share_pct: f64,
    active_ratio: f64,
    frames: usize,
}

fn read_mono(path: &Path) -> Result<Vec<f32>, AppError> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| AppError::Runtime(format!("打开 {} 失败: {e}", path.display())))?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample.clamp(1, 32);
            let scale = (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / scale)
                .collect()
        }
    };

    if ch == 1 {
        return Ok(interleaved);
    }
    Ok(interleaved
        .chunks(ch)
        .map(|c| c.iter().sum::<f32>() / ch as f32)
        .collect())
}

fn rms_db(x: f32) -> f64 {
    20.0 * (x.max(1e-9) as f64).log10()
}

fn analyze(vocals: &Path, inst: &Path) -> Result<Metrics, AppError> {
    let v = read_mono(vocals)?;
    let i = read_mono(inst)?;
    let n = v.len().min(i.len());
    if n == 0 {
        return Err(AppError::Runtime("分离结果为空".into()));
    }

    const FRAME: usize = 4410; // 100 ms @44.1k
    let frames = n / FRAME;

    let (mut ev, mut ei) = (0f64, 0f64);
    let mut peak_v = 0f64;
    let mut frame_v = Vec::with_capacity(frames);
    let mut frame_i = Vec::with_capacity(frames);

    for f in 0..frames {
        let s = f * FRAME;
        let sv = &v[s..s + FRAME];
        let si = &i[s..s + FRAME];
        let rv = (sv.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / FRAME as f64).sqrt();
        let ri = (si.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / FRAME as f64).sqrt();
        ev += rv * rv;
        ei += ri * ri;
        peak_v = peak_v.max(rv);
        frame_v.push(rv);
        frame_i.push(ri);
    }

    // 人声「活跃」帧：本帧人声既不能太弱（低于峰值 -34dB），也要压过伴奏
    let floor = peak_v * 10f64.powf(-34.0 / 20.0);
    let active = frame_v
        .iter()
        .zip(frame_i.iter())
        .filter(|(rv, ri)| **rv > floor && **rv > **ri * 0.18)
        .count();

    let share = if ev + ei > 0.0 { ev / (ev + ei) } else { 0.0 };

    Ok(Metrics {
        dur: n as f64 / 44100.0,
        vocal_rms_db: rms_db((ev / frames.max(1) as f64).sqrt() as f32),
        inst_rms_db: rms_db((ei / frames.max(1) as f64).sqrt() as f32),
        vocal_share_pct: share * 100.0,
        active_ratio: active as f64 / frames.max(1) as f64,
        frames,
    })
}

/// 双阈值：任一命中即判有人声。
/// 实测两个簇之间有明显空档（器乐 ≤0.4% / ≤1.8%，人声 ≥5.4% / ≥39.4%），
/// 阈值放在空档中间即可，不必精确。
fn verdict(m: &Metrics, share_th: f64, active_th: f64) -> (&'static str, &'static str) {
    let by_share = m.vocal_share_pct >= share_th;
    let by_active = m.active_ratio * 100.0 >= active_th;
    match (by_share, by_active) {
        (true, true) => ("vocal", "人声 + BGM"),
        (false, true) => ("vocal", "人声 + BGM（人声被编曲压得较低）"),
        (true, false) => ("vocal", "人声 + BGM（人声较断续）"),
        (false, false) => ("instrumental", "纯器乐（可作 BGM）"),
    }
}

/// 离阈值越远越可信。
fn confidence(m: &Metrics, share_th: f64, active_th: f64) -> &'static str {
    let r1 = m.vocal_share_pct / share_th.max(1e-9);
    let r2 = m.active_ratio * 100.0 / active_th.max(1e-9);
    let r = r1.max(r2); // >1 => 判为人声
    if r >= 3.0 || r <= 0.33 {
        "high"
    } else if r >= 1.6 || r <= 0.62 {
        "medium"
    } else {
        "low"
    }
}

// ---------------------------------------------------------------- main

fn run(app: &StemScan, ctx: &Context) -> Result<Value, AppError> {
    let t0 = Instant::now();
    let files = collect_inputs(&app.input, app.recursive)?;
    let sep = find_separator(&app.separator)?;
    let total = files.len();

    ctx.log(LogLevel::Info, format!("共 {total} 个音频，分离器: {sep}"));
    ctx.emit(Progress::Started {
        total: Some(total as u64),
        message: Some("开始扫描".into()),
    });

    let tmp = std::env::temp_dir().join("stemscan");
    std::fs::create_dir_all(&tmp)?;

    let share_th = app.threshold as f64;
    let active_th = app.active_threshold as f64;
    let mut results = Vec::new();

    for (idx, file) in files.iter().enumerate() {
        if ctx.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        ctx.tick((idx + 1) as u64, Some(total as u64), format!("分析 {name}"));

        let full_dur = probe_duration(&app.ffmpeg, file);

        // 1) 解码成 44.1k 立体声 wav（可选截断前 N 秒）
        let stem_dir = tmp.join(format!("run{idx}"));
        let _ = std::fs::remove_dir_all(&stem_dir);
        std::fs::create_dir_all(&stem_dir)?;
        let wav = stem_dir.join("input.wav");

        let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into()];
        if app.seconds > 0 {
            args.extend(["-t".into(), app.seconds.to_string()]);
        }
        args.extend([
            "-i".into(),
            file.to_string_lossy().to_string(),
            "-ac".into(),
            "2".into(),
            "-ar".into(),
            "44100".into(),
        ]);
        args.push(wav.to_string_lossy().to_string());
        run_cmd(&app.ffmpeg, &args)?;

        // 2) 音源分离
        let mut sargs: Vec<String> = vec![
            wav.to_string_lossy().to_string(),
            "--output_dir".into(),
            stem_dir.to_string_lossy().to_string(),
            "--output_format".into(),
            "WAV".into(),
        ];
        if !app.model.is_empty() {
            sargs.extend(["-m".into(), app.model.clone()]);
        }
        run_cmd(&sep, &sargs)?;

        // 3) 找 stems
        let mut voc: Option<PathBuf> = None;
        let mut ins: Option<PathBuf> = None;
        for e in std::fs::read_dir(&stem_dir)? {
            let p = e?.path();
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if !n.ends_with(".wav") || n == "input.wav" {
                continue;
            }
            if n.contains("vocal") {
                voc = Some(p);
            } else if n.contains("instrumental")
                || n.contains("accompaniment")
                || n.contains("no_vocal")
            {
                ins = Some(p);
            }
        }
        let (Some(vp), Some(ip)) = (voc, ins) else {
            ctx.log(LogLevel::Warn, format!("{name}: 分离产物缺失，跳过"));
            continue;
        };

        // 4) 统计
        let m = analyze(&vp, &ip)?;
        let (tag, label) = verdict(&m, share_th, active_th);
        let conf = confidence(&m, share_th, active_th);

        ctx.log(
            LogLevel::Info,
            format!(
                "{} → {}（人声占比 {:.1}% / 活跃 {:.0}%，可信度 {}）",
                name,
                label,
                m.vocal_share_pct,
                m.active_ratio * 100.0,
                conf
            ),
        );

        results.push(json!({
            "file": file.to_string_lossy(),
            "name": name,
            "verdict": tag,
            "label": label,
            "confidence": conf,
            "duration_sec": (full_dur * 10.0).round() / 10.0,
            "analyzed_sec": (m.dur * 10.0).round() / 10.0,
            "frames": m.frames,
            "vocal_share_pct": (m.vocal_share_pct * 10.0).round() / 10.0,
            "active_frame_ratio_pct": (m.active_ratio * 1000.0).round() / 10.0,
            "vocal_rms_db": (m.vocal_rms_db * 10.0).round() / 10.0,
            "instrumental_rms_db": (m.inst_rms_db * 10.0).round() / 10.0,
        }));

        if !app.keep_stems {
            let _ = std::fs::remove_dir_all(&stem_dir);
        }
    }

    let vocal_n = results.iter().filter(|r| r["verdict"] == "vocal").count();
    let inst_n = results
        .iter()
        .filter(|r| r["verdict"] == "instrumental")
        .count();

    let report = json!({
        "input": app.input.to_string_lossy(),
        "count": results.len(),
        "vocal": vocal_n,
        "instrumental": inst_n,
        "thresholds": { "vocal_share_pct": share_th, "active_frame_pct": active_th },
        "results": results,
    });

    if let Some(p) = &app.out {
        std::fs::write(p, serde_json::to_string_pretty(&report).unwrap_or_default())
            .map_err(|e| AppError::Runtime(format!("写 {} 失败: {e}", p.display())))?;
        ctx.log(LogLevel::Info, format!("报告已写入 {}", p.display()));
    }

    let ms = t0.elapsed().as_millis() as u64;
    ctx.done(report.clone(), ms);
    Ok(report)
}

fn main() {
    lilyco::run::<StemScan>();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec).expect("create wav");
        for s in samples {
            w.write_sample(*s).expect("write sample");
        }
        w.finalize().expect("finalize");
    }

    /// 前 half 有「人声」，后 half 静音 —— 用来验证活跃帧统计不是全 1 也不是全 0。
    fn half_tone(amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                if i < n / 2 {
                    amp * (i as f32 * 0.1).sin()
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn steady(amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    /// `active` 传 **0..1 的小数**（Metrics.active_ratio 就是这个量纲），
    /// `share` 传百分比。踩过一次：按百分比传 active，0.018 变成 1.8 => 1.8*100=180 全判 vocal。
    fn m(share: f64, active: f64) -> Metrics {
        Metrics {
            dur: 60.0,
            vocal_rms_db: -20.0,
            inst_rms_db: -15.0,
            vocal_share_pct: share,
            active_ratio: active,
            frames: 600,
        }
    }

    #[test]
    fn rms_db_handles_silence_without_neg_inf() {
        let v = rms_db(0.0);
        assert!(v.is_finite(), "silence must not produce -inf");
        assert!(v < -100.0);
        assert!((rms_db(1.0) - 0.0).abs() < 1e-9);
        assert!((rms_db(0.5) + 6.0206).abs() < 1e-3);
    }

    #[test]
    fn verdict_uses_either_metric() {
        // 两个指标都在空档里，任一命中即判 vocal
        assert_eq!(verdict(&m(20.0, 0.80), 2.0, 15.0).0, "vocal");
        assert_eq!(verdict(&m(5.0, 0.40), 2.0, 15.0).0, "vocal"); // 编曲压人声那种
        assert_eq!(verdict(&m(0.1, 0.018), 2.0, 15.0).0, "instrumental");
        assert_eq!(verdict(&m(0.0, 0.0), 2.0, 15.0).0, "instrumental");
    }

    #[test]
    fn confidence_grows_with_distance_from_threshold() {
        assert_eq!(confidence(&m(20.0, 0.80), 2.0, 15.0), "high");
        assert_eq!(confidence(&m(0.0, 0.0), 2.0, 15.0), "high");
        // 紧贴阈值 => 低可信度
        assert_eq!(confidence(&m(2.2, 0.155), 2.0, 15.0), "low");
    }

    #[test]
    fn analyze_separates_vocal_track_from_instrumental() {
        let dir = std::env::temp_dir().join("stemscan_test_analyze");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (v, i) = (dir.join("v.wav"), dir.join("i.wav"));

        let n = 44100 * 4; // 4s
        // 有人声：人声与伴奏同量级
        write_wav(&v, &half_tone(0.30, n));
        write_wav(&i, &steady(0.20, n));
        let mv = analyze(&v, &i).unwrap();
        assert!(mv.vocal_share_pct > 20.0, "share={}", mv.vocal_share_pct);
        assert!(mv.active_ratio > 0.3, "active={}", mv.active_ratio);
        assert_eq!(verdict(&mv, 2.0, 15.0).0, "vocal");

        // 纯器乐：人声轨只剩极微小的泄漏
        write_wav(&v, &half_tone(0.00005, n));
        let mi = analyze(&v, &i).unwrap();
        assert!(mi.vocal_share_pct < 1.0, "share={}", mi.vocal_share_pct);
        assert_eq!(verdict(&mi, 2.0, 15.0).0, "instrumental");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_inputs_filters_non_audio_and_respects_recursion() {
        let dir = std::env::temp_dir().join("stemscan_test_collect");
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        for p in [
            dir.join("a.mp3"),
            dir.join("b.flac"),
            dir.join("notes.txt"),
            dir.join("x.ncm"),
            sub.join("c.wav"),
        ] {
            std::fs::write(&p, b"x").unwrap();
        }

        let flat = collect_inputs(&dir, false).unwrap();
        assert_eq!(flat.len(), 2, "非递归只收顶层音频: {flat:?}");

        let deep = collect_inputs(&dir, true).unwrap();
        assert_eq!(deep.len(), 3, "递归要收子目录: {deep:?}");

        assert!(collect_inputs(&dir.join("a.mp3"), false).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_separator_honours_explicit_path() {
        let got = find_separator(&Some("C:/nope/audio-separator.exe".into())).unwrap();
        assert_eq!(got, "C:/nope/audio-separator.exe");
    }
}
