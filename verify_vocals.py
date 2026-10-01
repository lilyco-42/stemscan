# -*- coding: utf-8 -*-
"""Ground-truth cross-check: transcribe the separated VOCALS stem with faster-whisper.
A real vocal track should yield lyrics; an instrumental stem should yield little or nothing."""
import json, os, re, subprocess, sys
import numpy as np
from faster_whisper import WhisperModel

STEM = os.path.join(os.environ.get("TEMP", "/tmp"), "stemscan")
REPORT = sys.argv[1] if len(sys.argv) > 1 else "report_raw.json"

rep = json.load(open(REPORT, encoding="utf-8"))
names = [r["name"] for r in rep["results"]]

model = WhisperModel("large-v3-turbo", device="cpu", compute_type="int8")

# whisper's favourite hallucinations on music / silence
HALLUC = ["感谢观看", "谢谢观看", "字幕由", "请不吝点赞", "订阅", "amara.org", "字幕志愿者",
          "Thanks for watching", "Thank you for watching", "字幕", "MING PAO", "由"]

print(f"{'文件':<40} {'字数':>5} {'幻觉':>5}  转写片段")
print("-" * 110)
out = []
for i, name in enumerate(names):
    wav = os.path.join(STEM, f"run{i}", "input_(Vocals)_1_HP-UVR.wav")
    if not os.path.exists(wav):
        print(f"{name[:38]:<40}  (缺 stem)")
        continue
    pcm = os.path.join(STEM, "_v.f32")
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", wav, "-ac", "1", "-ar", "16000",
                    "-f", "f32le", "-acodec", "pcm_f32le", pcm], check=True)
    a = np.fromfile(pcm, dtype=np.float32)
    segs, _ = model.transcribe(a, language=None, beam_size=1, vad_filter=True,
                               condition_on_previous_text=False)
    txt = "".join(s.text for s in segs).strip()
    clean = re.sub(r"[^\w\u4e00-\u9fff]", "", txt)
    halluc = sum(txt.count(h) for h in HALLUC)
    flag = "HALL" if halluc and len(clean) < 40 else ("-" if len(clean) >= 8 else "空")
    print(f"{name[:38]:<40} {len(clean):>5} {halluc:>5}  {txt[:60]}")
    out.append({"name": name, "chars": len(clean), "halluc": halluc, "text": txt[:200]})

json.dump(out, open("verify_vocals.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print("\n-> verify_vocals.json")
