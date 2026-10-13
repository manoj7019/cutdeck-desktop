// End-to-end UI test: drives the real Cutdeck app (Rust backend + webview) through
// tauri-driver's WebDriver endpoint. Run via tests/run-e2e.sh.
import { writeFileSync, existsSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";

const APP = process.env.CUTDECK_APP;
const MEDIA = process.env.CUTDECK_MEDIA; // folder with long.mp4, portrait.mp4, green.png, music.mp3
const OUT = process.env.CUTDECK_TEST_SAVE_DIR;
const WD = "http://127.0.0.1:4444";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log("•", ...a);

async function wd(method, path, body) {
  const r = await fetch(WD + path, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  const j = await r.json();
  if (j.value && j.value.error) throw new Error(`${path}: ${j.value.error} ${j.value.message}`);
  return j.value;
}
let sid;
const exec = (script, args = []) => wd("POST", `/session/${sid}/execute/sync`, { script, args });
const execAsync = (script, args = []) => wd("POST", `/session/${sid}/execute/async`, { script, args });
const find = (css) => wd("POST", `/session/${sid}/element`, { using: "css selector", value: css }).then((e) => Object.values(e)[0]);
const click = async (css) => wd("POST", `/session/${sid}/element/${await find(css)}/click`, {});
const shot = async (name) => { const b64 = await wd("GET", `/session/${sid}/screenshot`); writeFileSync(`${OUT}/${name}.png`, Buffer.from(b64, "base64")); log("screenshot", name); };
async function until(fn, ms = 60000, what = "condition") {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) { const v = await fn(); if (v) return v; await sleep(250); }
  throw new Error("Timed out waiting for " + what);
}
function assert(c, m) { if (!c) throw new Error("ASSERT: " + m); log("ok -", m); }

const s = await wd("POST", "/session", { capabilities: { alwaysMatch: { "tauri:options": { application: APP } } } });
sid = s.sessionId;
try {
  await until(() => exec("return typeof S!=='undefined' && S.clips.length===2"), 30000, "editor boot");
  await exec("document.querySelector('#app').style.height='100vh'");
  const env = await until(() => exec("return ENV"), 60000, "env check");
  assert(env.ok, `FFmpeg found (${env.encoder}, ${env.version})`);
  await shot("01-start");

  // import through the real media:// protocol
  const n = await execAsync(
    "const done=arguments[arguments.length-1];importFiles(arguments[0]).then(()=>done(S.clips.length)).catch(e=>done('ERR '+e))",
    [[`${MEDIA}/long.mp4`, `${MEDIA}/portrait.mp4`, `${MEDIA}/green.png`]]);
  assert(n === 3, `imported 3 clips (got ${n})`);
  const m0 = await exec("const m=S.media.get(S.clips[0].mediaId);return {w:m.w,h:m.h,dur:m.dur,proxy:m.proxy}");
  assert(m0.w === 640 && Math.round(m0.dur) === 120, `video metadata read through media:// (${JSON.stringify(m0)})`);
  await until(() => exec("return S.media.get(S.clips[0].mediaId).thumbs.length>3"), 30000, "thumbnails");

  // UI: 100x on clip 1 via the Speed tool
  await exec("S.sel={type:'clip',id:S.clips[0].id};renderTimeline()");
  await click('[data-tool="speed"]');
  await click('[data-sp="100"]');
  const sp = await exec("return [S.clips[0].speed, clipLen(S.clips[0]), S.clips[0].vol]");
  assert(sp[0] === 100 && Math.abs(sp[1] - 1.2) < 1e-6 && sp[2] === 0, `100x applied: 120s -> ${sp[1]}s, sound auto-muted`);

  // slider path: speed 10x on clip 2 (portrait 6s -> 0.6s); then 0.5x through the exact box
  await exec("S.sel={type:'clip',id:S.clips[1].id};renderPanel()");
  await click('[data-sp="2"]');
  assert(await exec("return S.clips[1].speed") === 2, "2x via chip on clip 2");

  // green screen on the photo
  await exec("S.sel={type:'clip',id:S.clips[2].id}");
  await click('[data-tool="cutout"]');
  await click('[data-bm="chroma"]');
  await click('[data-br="color"]');
  assert(await exec("return S.clips[2].bgMode+'/'+S.clips[2].bgReplace") === "chroma/color", "green screen on photo clip");

  // filter + text + music
  await exec("S.sel={type:'clip',id:S.clips[1].id}");
  await click('[data-tool="filter"]');
  await click('[data-f="vintage"]');
  await exec("seek(0.2)");
  await click("#bAddText");
  await exec("const t=S.texts[S.texts.length-1];t.text='CUTDECK 100x';t.style='box';t.box='#ffd23f';t.color='#111111';t.y=0.15;renderPanel();requestStill()");
  const mus = await execAsync("const d=arguments[arguments.length-1];addMusic(arguments[0]).then(()=>d(!!S.music))", [`${MEDIA}/music.mp3`]);
  assert(mus, "music added");
  await exec("seek(0.5)");
  await sleep(800);
  await shot("02-edited");

  // playback advances (100x clip uses frame stepping)
  await click("#bPlay");
  await sleep(1500);
  const T = await exec("return S.T");
  await shot("03-playing");
  await click("#bPlay");
  assert(T > 0.6, `playback advanced to ${T.toFixed(2)}s`);

  // undo/redo
  const before = await exec("return S.clips.length");
  await exec("S.sel={type:'clip',id:S.clips[2].id};delSel()");
  await exec("undo()");
  assert(await exec("return S.clips.length") === before, "delete + undo restores clip");

  // project save
  const saved = await execAsync("const d=arguments[arguments.length-1];saveProject(false).then(r=>d(r))");
  assert(saved === true && existsSync(`${OUT}/My video.cutdeck`), "project saved to .cutdeck file");

  // export through FFmpeg
  await click("#bExport");
  await until(() => exec("return !!document.querySelector('#exGo') && !document.querySelector('#exGo').disabled"), 30000, "export sheet");
  await click('[data-res="720"]');
  await shot("04-export-sheet");
  await click("#exGo");
  await until(() => exec("return document.querySelector('#exBar')!==null || /ready|failed/.test(document.querySelector('#exSheet h3').textContent)"), 20000, "export start");
  await sleep(700);
  await shot("05-exporting");
  const title = await until(async () => { const t = await exec("return document.querySelector('#exSheet h3').textContent"); return /ready|failed/.test(t) ? t : null; }, 300000, "export finish");
  const note = await exec("return document.querySelector('#exSheet').innerText");
  await shot("06-export-done");
  assert(/ready/.test(title), "export finished: " + note.replace(/\n+/g, " | "));

  const out = execFileSync("ls", [OUT]).toString().split("\n").find((f) => f.endsWith(".mp4"));
  const probe = JSON.parse(execFileSync("ffprobe", ["-v", "error", "-print_format", "json", "-show_streams", "-show_format", `${OUT}/${out}`]).toString());
  const v = probe.streams.find((x) => x.codec_type === "video"), a = probe.streams.find((x) => x.codec_type === "audio");
  const expected = await exec("return total()");
  log("output", out, v.width + "x" + v.height, probe.format.duration + "s", (statSync(`${OUT}/${out}`).size / 1e6).toFixed(2) + " MB");
  assert(v.width === 720 && v.height === 1280, "9:16 720p output");
  assert(!!a, "has audio track");
  assert(Math.abs(+probe.format.duration - expected) < 0.2, `duration ${probe.format.duration} matches timeline ${expected.toFixed(2)}`);
  execFileSync("ffmpeg", ["-loglevel", "error", "-y", "-i", `${OUT}/${out}`, "-vf", "fps=2,scale=160:-2,tile=8x1", "-frames:v", "1", `${OUT}/07-export-frames.png`]);

  // AI cutout: native ONNX Runtime in Rust, live preview + export
  const envNow = await exec("return ENV");
  assert(envNow.cutout, "native AI cutout available: " + (envNow.cutoutMessage || "ok"));
  await exec("document.querySelector('#exClose2').click();S.clips[1].bgMode='person';S.clips[1].bgReplace='color';S.clips[1].bgColor='#ff00ff'");
  await exec("EX.res=720");
  await click("#bExport");
  await until(() => exec("return !!document.querySelector('#exGo') && !document.querySelector('#exGo').disabled"), 30000, "export sheet 2");
  const before2 = Date.now();
  await click("#exGo");
  const title2 = await until(async () => { const t = await exec("return document.querySelector('#exSheet h3').textContent"); return /ready|failed/.test(t) ? t : null; }, 600000, "cutout export");
  const note2 = await exec("return document.querySelector('#exSheet').innerText");
  assert(/ready/.test(title2), `export with AI cutout clip finished in ${((Date.now() - before2) / 1000).toFixed(1)}s: ` + note2.replace(/\n+/g, " | "));
  // transparent PNG from a photo clip
  await exec("document.querySelector('#exClose2')&&document.querySelector('#exClose2').click();S.clips[2].bgMode='person';S.sel={type:'clip',id:S.clips[2].id};S.tool='cutout';renderRail();renderPanel()");
  await click("#bPng");
  await until(() => existsSync(`${OUT}/green-cutout.png`), 60000, "cutout PNG");
  await sleep(500);
  const pngInfo = execFileSync("ffprobe", ["-v", "error", "-show_entries", "stream=width,height,pix_fmt", "-of", "csv=p=0", `${OUT}/green-cutout.png`]).toString().trim();
  assert(/800,600,rgba/.test(pngInfo), "transparent PNG saved at full size: " + pngInfo);
  await exec("seek(clipStart(S.clips[1].id)+0.3)");
  await sleep(1500);
  await shot("08-cutout-preview");
  log("ALL PASSED");
} finally {
  await wd("DELETE", `/session/${sid}`).catch(() => {});
}
