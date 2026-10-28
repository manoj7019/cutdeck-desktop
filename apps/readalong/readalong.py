"""Readalong: a desktop PDF reader that reads aloud with a neural voice
and highlights each word and sentence in exact sync.

PDF engine: PyMuPDF. Voice: Kokoro-82M (int8) on ONNX Runtime, with the
model's own per-phoneme durations used for word timing. Window: the
system web view (WebView2 on Windows, WebKit on macOS) via pywebview.
"""
from __future__ import annotations

import base64
import io
import json
import os
import re
import sys
import threading
import time
import traceback
import unicodedata
import urllib.request
import wave
from collections import OrderedDict
from pathlib import Path

import numpy as np

APP_DIR = Path(__file__).resolve().parent
APP_NAME = "Readalong"


def data_dir() -> Path:
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local")
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share")
    d = base / APP_NAME
    d.mkdir(parents=True, exist_ok=True)
    return d


DATA = data_dir()
STATE_FILE = DATA / "state.json"
MODEL_URL = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.int8.onnx"
VOICES_URL = "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin"
MODEL_RAW = DATA / "kokoro-v1.0.int8.onnx"
MODEL_TIMED = DATA / "kokoro-v1.0.int8.timed.onnx"
VOICES_FILE = DATA / "voices-v1.0.bin"
SR = 24000
FRAME = 600  # audio samples per duration frame
MAX_TOKENS = 500


def log(*a):
    try:
        with open(DATA / "readalong.log", "a", encoding="utf-8") as f:
            f.write(time.strftime("%H:%M:%S ") + " ".join(str(x) for x in a) + "\n")
    except Exception:
        pass


# --------------------------------------------------------------------------- state
class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.data = {"settings": {}, "books": {}}
        try:
            self.data.update(json.loads(STATE_FILE.read_text(encoding="utf-8")))
        except Exception:
            pass

    def save(self):
        with self.lock:
            tmp = STATE_FILE.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.data, indent=1), encoding="utf-8")
            tmp.replace(STATE_FILE)


# --------------------------------------------------------------------------- voice
class Voice:
    """Downloads, prepares and runs the Kokoro model."""

    def __init__(self):
        self.state = "idle"  # idle, downloading, preparing, loading, ready, error
        self.progress = 0.0
        self.total_mb = 0
        self.error = ""
        self.sess = None
        self.voices = None
        self.tok = None
        self.ph_cache: dict = {}
        self.run_lock = threading.Lock()
        self.synth_lock = threading.RLock()
        self._started = False

    def start(self):
        if self._started:
            return
        self._started = True
        threading.Thread(target=self._load, daemon=True).start()

    def _download(self, url: str, dest: Path, weight: tuple[float, float]):
        if dest.exists() and dest.stat().st_size > 1_000_000:
            return
        tmp = dest.with_suffix(dest.suffix + ".part")
        req = urllib.request.Request(url, headers={"User-Agent": "Readalong"})
        with urllib.request.urlopen(req, timeout=60) as r, open(tmp, "wb") as f:
            total = int(r.headers.get("Content-Length") or 0)
            got = 0
            while True:
                chunk = r.read(1 << 16)
                if not chunk:
                    break
                f.write(chunk)
                got += len(chunk)
                if total:
                    self.progress = weight[0] + (weight[1] - weight[0]) * got / total
        tmp.replace(dest)

    def _load(self):
        try:
            if not MODEL_TIMED.exists() or not VOICES_FILE.exists():
                self.state = "downloading"
                self.total_mb = 120
                self._download(VOICES_URL, VOICES_FILE, (0.0, 0.23))
                self._download(MODEL_URL, MODEL_RAW, (0.23, 1.0))
                self.state = "preparing"
                self._add_duration_output()
            self.state = "loading"
            import onnxruntime as ort
            from kokoro_onnx.tokenizer import Tokenizer

            so = ort.SessionOptions()
            so.intra_op_num_threads = max(1, (os.cpu_count() or 4) - 1)
            self.sess = ort.InferenceSession(str(MODEL_TIMED), so, providers=["CPUExecutionProvider"])
            self.voices = np.load(str(VOICES_FILE))
            self.tok = Tokenizer()
            self.state = "ready"
            log("voice ready")
        except Exception as e:
            self.error = f"{type(e).__name__}: {e}"
            self.state = "error"
            log("voice error", traceback.format_exc())

    @staticmethod
    def _add_duration_output():
        """Expose the duration predictor so we know when each phoneme is spoken."""
        import onnx

        m = onnx.load(str(MODEL_RAW))
        names = {o.name for o in m.graph.output}
        if "duration" not in names:
            src = None
            for n in m.graph.node:
                if n.op_type == "Clip" and n.name.endswith("/encoder/Clip"):
                    src = n.output[0]
                    break
            if src is None:
                raise RuntimeError("Could not find the duration layer in the voice model.")
            m.graph.node.append(onnx.helper.make_node("Identity", [src], ["duration"], name="readalong_duration"))
            m.graph.output.append(onnx.helper.make_tensor_value_info("duration", onnx.TensorProto.FLOAT, None))
        onnx.save(m, str(MODEL_TIMED))
        try:
            MODEL_RAW.unlink()
        except Exception:
            pass

    # ---- synthesis
    def _phon(self, word: str, lang: str) -> str:
        key = (lang, word)
        p = self.ph_cache.get(key)
        if p is None:
            try:
                p = self.tok.phonemize(word, lang)
            except Exception:
                p = ""
            if len(self.ph_cache) > 50000:
                self.ph_cache.clear()
            self.ph_cache[key] = p
        return p

    def _phonemes_for(self, words: list[str], lang: str) -> list[str]:
        """Phonemize the phrase as a whole for natural stress; fall back per word
        when the phonemizer changes the word count (numbers, abbreviations)."""
        whole = ""
        try:
            whole = self.tok.phonemize(" ".join(words), lang)
        except Exception:
            pass
        parts = whole.split()
        if len(parts) == len(words):
            return parts
        return [self._phon(w, lang) for w in words]

    def synth(self, words: list[str], voice: str) -> tuple[np.ndarray, list[float], list[float]]:
        """Returns audio, start time per word, end time per word (seconds)."""
        with self.synth_lock:
            return self._synth(words, voice)

    def _synth(self, words: list[str], voice: str) -> tuple[np.ndarray, list[float], list[float]]:
        lang = "en-gb" if voice.startswith("b") else "en-us"
        phs = self._phonemes_for(words, lang)
        tokens: list[int] = []
        spans = []
        for k, ph in enumerate(phs):
            t = self.tok.tokenize(ph + (" " if k < len(phs) - 1 else "")) if ph else []
            spans.append((len(tokens), len(tokens) + len(t)))
            tokens += t
        if len(tokens) > MAX_TOKENS and len(words) > 1:
            h = len(words) // 2
            a1, s1, e1 = self._synth(words[:h], voice)
            a2, s2, e2 = self._synth(words[h:], voice)
            off = len(a1) / SR
            return np.concatenate([a1, a2]), s1 + [x + off for x in s2], e1 + [x + off for x in e2]
        if not tokens:
            n = len(words)
            return np.zeros(int(SR * 0.2), np.float32), [0.0] * n, [0.2] * n
        tokens = tokens[:MAX_TOKENS]
        vs = self.voices[voice]
        style = vs[min(len(tokens), len(vs)) - 1].reshape(1, -1).astype(np.float32)
        feeds = {
            "tokens": np.array([[0, *tokens, 0]], dtype=np.int64),
            "style": style,
            "speed": np.array([1.0], dtype=np.float32),
        }
        with self.run_lock:
            audio, dur = self.sess.run(None, feeds)
        audio = np.asarray(audio, dtype=np.float32).ravel()
        dur = np.asarray(dur, dtype=np.float64).ravel()
        cum = np.concatenate([[0.0], np.cumsum(dur)]) * FRAME / SR
        starts, ends = [], []
        for s, e in spans:
            s1 = min(s + 1, len(cum) - 1)
            e1 = min(e + 1, len(cum) - 1)
            starts.append(float(cum[s1]))
            # end of the word ignores the trailing space token
            ends.append(float(cum[max(s1, e1 - 1)] if e > s else cum[s1]))
        return audio, starts, ends


# --------------------------------------------------------------------------- document
ABBR = set("mr. mrs. ms. dr. prof. sr. jr. st. vs. etc. e.g. i.e. fig. figs. no. vol. pp. al. inc. ltd. co. approx. dept. eq. eqs. ch. sec. ref. refs. cf. ca.".split())
TERM = re.compile(r"[.!?…][\"'”’)\]]*$")
SOFT = re.compile(r"[,;:—–)]$")
QUOTES = str.maketrans({"“": '"', "”": '"', "‘": "'", "’": "'", "­": ""})


def speakable(t: str) -> str:
    t = unicodedata.normalize("NFKC", t).translate(QUOTES)
    return t


class Document:
    def __init__(self, path: str):
        import pymupdf as fitz

        self.path = path
        self.name = Path(path).name
        self.lock = threading.Lock()
        self.doc = fitz.open(path)
        if self.doc.needs_pass:
            raise RuntimeError("This PDF is password protected.")
        self.n = self.doc.page_count
        self.sizes = []
        for i in range(self.n):
            r = self.doc[i].rect
            self.sizes.append([round(r.width, 2), round(r.height, 2)])
        self.text: list[str] = []
        self.page: list[int] = []
        self.rects: list[list] = []
        self.hgt: list[float] = []
        self.skip: list[bool] = []
        self.page_start = [0] * (self.n + 1)
        self.indexed = 0
        self.sent: list[int] = []
        self.units: list[list[int]] = []
        self.word_unit: list[int] = []
        self.done = False
        self.error = ""
        self.img_cache: OrderedDict = OrderedDict()
        threading.Thread(target=self._index, daemon=True).start()

    def _index(self):
        try:
            for pi in range(self.n):
                with self.lock:
                    pg = self.doc[pi]
                    W, H = pg.rect.width, pg.rect.height
                    words = pg.get_text("words")
                self.page_start[pi] = len(self.text)
                lines: dict = {}
                for w in words:
                    lines.setdefault((w[5], w[6]), []).append(w)
                skip_lines = set()
                for key, lw in lines.items():
                    y0 = min(w[1] for w in lw)
                    y1 = max(w[3] for w in lw)
                    yc = (y0 + y1) / 2
                    if (yc < H * 0.075 or yc > H * 0.925) and len(lw) <= 8:
                        skip_lines.add(key)
                prev_key = None
                for w in words:
                    x0, y0, x1, y1, t = w[0], w[1], w[2], w[3], w[4]
                    key = (w[5], w[6])
                    rect = [x0 / W, y0 / H, max(0.5, x1 - x0) / W, max(0.5, y1 - y0) / H]
                    if (
                        self.text
                        and self.page[-1] == pi
                        and prev_key is not None
                        and key != prev_key
                        and re.search(r"\w-$", self.text[-1])
                        and re.match(r"[a-z]", t)
                    ):
                        self.text[-1] = self.text[-1][:-1] + t
                        self.rects[-1].append(rect)
                        prev_key = key
                        continue
                    self.text.append(t)
                    self.page.append(pi)
                    self.rects.append([rect])
                    self.hgt.append((y1 - y0) / H)
                    self.skip.append(key in skip_lines)
                    prev_key = key
                self.indexed = pi + 1
            self.page_start[self.n] = len(self.text)
            self._structure()
            self.done = True
        except Exception as e:
            self.error = str(e)
            log("index error", traceback.format_exc())

    def _structure(self):
        idx = [i for i in range(len(self.text)) if not self.skip[i]]
        if not idx:  # everything looked like a header: read it all
            idx = list(range(len(self.text)))
            self.skip = [False] * len(self.text)
        self.sent = [-1] * len(self.text)
        sentences: list[list[int]] = []
        cur: list[int] = []
        for k, i in enumerate(idx):
            cur.append(i)
            j = idx[k + 1] if k + 1 < len(idx) else None
            end = j is None
            if not end:
                a, b = self.text[i], self.text[j]
                term = bool(TERM.search(a)) and a.lower().rstrip("\"'”’)]") not in ABBR and not re.fullmatch(r"[A-Z]\.", a)
                if term and not re.match(r"[a-z]", b):
                    end = True
                elif self.page[j] != self.page[i]:
                    end = term
                else:
                    ra, rb = self.rects[i][-1], self.rects[j][0]
                    gap = rb[1] - (ra[1] + ra[3])
                    ha = self.hgt[i]
                    if gap > ha * 0.9:
                        end = True
                    elif abs(self.hgt[j] - ha) > ha * 0.15 and gap > -ha * 0.3:
                        end = True
            if end:
                sentences.append(cur)
                cur = []
        for si, s in enumerate(sentences):
            for i in s:
                self.sent[i] = si
        self.word_unit = [-1] * len(self.text)
        units = []
        for s in sentences:
            part, chars = [], 0
            for k, i in enumerate(s):
                part.append(i)
                chars += len(self.text[i]) + 1
                last = k == len(s) - 1
                if last or (chars > 170 and SOFT.search(self.text[i])) or chars > 280:
                    units.append(part)
                    part, chars = [], 0
        for ui, u in enumerate(units):
            for i in u:
                self.word_unit[i] = ui
        self.units = units

    def render(self, i: int, px: int) -> str:
        import pymupdf as fitz

        px = max(200, min(int(px), 3200))
        key = (i, px)
        if key in self.img_cache:
            self.img_cache.move_to_end(key)
            return self.img_cache[key]
        with self.lock:
            pg = self.doc[i]
            z = px / pg.rect.width
            pix = pg.get_pixmap(matrix=fitz.Matrix(z, z), alpha=False)
            data = pix.tobytes("png")
        url = "data:image/png;base64," + base64.b64encode(data).decode("ascii")
        self.img_cache[key] = url
        while len(self.img_cache) > 24:
            self.img_cache.popitem(last=False)
        return url


# --------------------------------------------------------------------------- speech queue
class Speaker:
    """Generates audio for reading units, one at a time, newest request first."""

    def __init__(self, voice: Voice):
        self.voice = voice
        self.cv = threading.Condition()
        self.jobs: list = []  # [key, unit_words, voice, event, holder]
        self.cache: OrderedDict = OrderedDict()
        self.gen = 0
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self):
        while True:
            with self.cv:
                while not self.jobs:
                    self.cv.wait()
                job = self.jobs.pop(0)
            key, words, texts, voice, ev, holder = job
            if key in self.cache:
                holder["r"] = self.cache[key]
                ev.set()
                continue
            try:
                while self.voice.state not in ("ready", "error"):
                    time.sleep(0.2)
                if self.voice.state == "error":
                    raise RuntimeError(self.voice.error)
                audio, st, en = self.voice.synth(texts, voice)
                r = {"wav": wav_b64(audio), "words": words, "start": st, "ends": en, "dur": len(audio) / SR}
                with self.cv:
                    self.cache[key] = r
                    while len(self.cache) > 16:
                        self.cache.popitem(last=False)
                holder["r"] = r
            except Exception as e:
                holder["err"] = str(e)
                log("synth error", traceback.format_exc())
            ev.set()

    def request(self, key, words, texts, voice, priority=False):
        with self.cv:
            if key in self.cache:
                ev = threading.Event()
                ev.set()
                return ev, {"r": self.cache[key]}
            for j in self.jobs:
                if j[0] == key:
                    if priority:
                        self.jobs.remove(j)
                        self.jobs.insert(0, j)
                    return j[4], j[5]
            ev, holder = threading.Event(), {}
            job = [key, words, texts, voice, ev, holder]
            if priority:
                self.jobs.insert(0, job)
            else:
                self.jobs.append(job)
            self.cv.notify()
            return ev, holder

    def drop_pending(self):
        with self.cv:
            for j in self.jobs:
                j[5]["err"] = "dropped"
                j[4].set()
            self.jobs.clear()


def wav_b64(a: np.ndarray) -> str:
    pcm = (np.clip(a, -1, 1) * 32767).astype("<i2").tobytes()
    bio = io.BytesIO()
    with wave.open(bio, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm)
    return base64.b64encode(bio.getvalue()).decode("ascii")


# --------------------------------------------------------------------------- API for the page
class Api:
    def __init__(self):
        self._state = State()
        self._voice = Voice()
        self._speaker = Speaker(self._voice)
        self._doc: Document | None = None
        self._window = None
        self._doc_id = 0

    # settings / library
    def get_settings(self):
        return self._state.data.get("settings", {})

    def set_settings(self, s):
        self._state.data["settings"] = s
        self._state.save()
        return True

    def recent(self):
        books = self._state.data.get("books", {})
        out = []
        for p, b in sorted(books.items(), key=lambda kv: -kv[1].get("ts", 0))[:8]:
            if os.path.exists(p):
                out.append({"path": p, "name": b.get("name") or Path(p).name, "page": b.get("page", 1), "pages": b.get("pages", 0), "pct": b.get("pct", 0)})
        return out

    def forget(self, path):
        self._state.data.get("books", {}).pop(path, None)
        self._state.save()
        return True

    def save_pos(self, word, page, pct):
        d = self._doc
        if not d:
            return False
        b = self._state.data.setdefault("books", {}).setdefault(d.path, {})
        b.update({"name": d.name, "word": int(word), "page": int(page), "pages": d.n, "pct": float(pct), "ts": time.time()})
        self._state.save()
        return True

    # voice
    def voice_status(self):
        v = self._voice
        v.start()
        return {"state": v.state, "progress": round(v.progress, 3), "mb": v.total_mb, "error": v.error}

    # documents
    def choose_file(self):
        import webview

        kind = getattr(getattr(webview, "FileDialog", None), "OPEN", None) or webview.OPEN_DIALOG
        res = self._window.create_file_dialog(kind, allow_multiple=False, file_types=("PDF files (*.pdf)", "All files (*.*)"))
        if not res:
            return None
        return res[0] if isinstance(res, (list, tuple)) else res

    def open(self, path):
        try:
            self._speaker.drop_pending()
            if self._doc:
                try:
                    self._doc.doc.close()
                except Exception:
                    pass
            self._doc = Document(path)
            self._doc_id += 1
            b = self._state.data.get("books", {}).get(path, {})
            return {"ok": True, "id": self._doc_id, "name": self._doc.name, "path": path, "sizes": self._doc.sizes, "resume": b.get("word", 0)}
        except Exception as e:
            log("open error", traceback.format_exc())
            return {"ok": False, "error": str(e)}

    def index_status(self):
        d = self._doc
        if not d:
            return None
        return {"indexed": d.indexed, "total": d.n, "done": d.done, "error": d.error}

    def doc_map(self):
        d = self._doc
        if not d or not d.done:
            return None
        return {"count": len(d.text), "sent": d.sent, "pageStart": d.page_start, "skip": [i for i, s in enumerate(d.skip) if s]}

    def page(self, i, px):
        d = self._doc
        if not d:
            return None
        return {"img": d.render(int(i), px)}

    def startup_path(self):
        if len(sys.argv) > 1 and sys.argv[1].lower().endswith(".pdf") and os.path.exists(sys.argv[1]):
            return os.path.abspath(sys.argv[1])
        return None

    def page_rects(self, i):
        d = self._doc
        if not d or not d.done:
            return None
        i = int(i)
        s, e = d.page_start[i], d.page_start[i + 1]
        return {"start": s, "rects": d.rects[s:e]}

    def _unit_job(self, start_word, voice, priority):
        d = self._doc
        ui = d.word_unit[start_word]
        if ui < 0:
            # a skipped word (header): move to the next readable unit
            nxt = next((d.word_unit[j] for j in range(start_word, len(d.text)) if d.word_unit[j] >= 0), -1)
            if nxt < 0:
                return None, None, None
            ui = nxt
            start_word = d.units[ui][0]
        unit = d.units[ui]
        words = [i for i in unit if i >= start_word]
        texts = [speakable(d.text[i]) for i in words]
        key = (self._doc_id, words[0], words[-1], voice)
        ev, holder = self._speaker.request(key, words, texts, voice, priority)
        return ui, ev, holder

    def speak(self, start_word, voice, doc_id):
        """Audio for the reading unit that contains start_word, from that word on."""
        d = self._doc
        if not d or not d.done or doc_id != self._doc_id:
            return {"error": "stale"}
        start_word = int(start_word)
        if start_word >= len(d.text):
            return {"finished": True}
        ui, ev, holder = self._unit_job(start_word, voice, True)
        if ev is None:
            return {"finished": True}
        # prefetch the next two units
        for k in (1, 2):
            if ui + k < len(d.units):
                self._unit_job(d.units[ui + k][0], voice, False)
        ev.wait()
        if "err" in holder:
            return {"error": holder["err"]}
        r = dict(holder["r"])
        r["next"] = d.units[ui + 1][0] if ui + 1 < len(d.units) else -1
        return r

    def sample(self, voice):
        v = self._voice
        v.start()
        while v.state not in ("ready", "error"):
            time.sleep(0.2)
        if v.state == "error":
            return {"error": "The voice couldn't load: " + v.error}
        try:
            a, _, _ = v.synth("Hello! This is how I will sound while reading your documents.".split(), voice)
            return {"wav": wav_b64(a)}
        except Exception as e:
            return {"error": str(e)}

    def cancel(self):
        self._speaker.drop_pending()
        return True

    def reveal_log(self):
        return str(DATA / "readalong.log")


def main():
    import webview

    api = Api()
    win = webview.create_window(
        APP_NAME,
        url=str(APP_DIR / "ui.html"),
        js_api=api,
        width=1280,
        height=880,
        min_size=(720, 560),
        background_color="#E7EAEF",
        text_select=True,
    )
    api._window = win

    def bind(w):
        api._voice.start()
        try:
            from webview.dom import DOMEventHandler

            def on_drop(e):
                try:
                    files = e.get("dataTransfer", {}).get("files", [])
                    for f in files:
                        p = f.get("pywebviewFullPath")
                        if p and p.lower().endswith(".pdf"):
                            w.evaluate_js(f"window.appOpenPath && window.appOpenPath({json.dumps(p)})")
                            break
                except Exception:
                    log("drop error", traceback.format_exc())

            w.dom.document.events.drop += DOMEventHandler(on_drop, True, True)
        except Exception:
            log("drop bind failed", traceback.format_exc())

    webview.start(bind, win, debug=bool(os.environ.get("READALONG_DEBUG")))
    os._exit(0)  # end background voice threads cleanly with the window


if __name__ == "__main__":
    main()
