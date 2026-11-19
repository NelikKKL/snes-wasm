import init, { Emulator } from './pkg/snes_web.js';

const W = 256, H = 224;
const STEP = 1000 / 60.0988; // NTSC

// [имя, бит в маске JOY1]
const BUTTONS = [
  ['Up', 11], ['Down', 10], ['Left', 9], ['Right', 8],
  ['A', 7], ['B', 15], ['X', 6], ['Y', 14],
  ['L', 5], ['R', 4], ['Start', 12], ['Select', 13],
];
const DEFAULTS = {
  Up: 'ArrowUp', Down: 'ArrowDown', Left: 'ArrowLeft', Right: 'ArrowRight',
  A: 'KeyX', B: 'KeyZ', X: 'KeyS', Y: 'KeyA',
  L: 'KeyD', R: 'KeyC', Start: 'Enter', Select: 'ShiftRight',
};
const STORE_KEY = 'snes-binds-v1';

let binds = loadBinds();
let listening = null;
const down = new Set();
const btnEls = {};

function loadBinds() {
  try { return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(STORE_KEY) || '{}') }; }
  catch { return { ...DEFAULTS }; }
}
function saveBinds() {
  try { localStorage.setItem(STORE_KEY, JSON.stringify(binds)); } catch {}
}
function keyLabel(code) {
  const arrows = { ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' };
  return arrows[code] ?? code.replace(/^(Key|Digit)/, '');
}

function renderBinds() {
  const box = document.getElementById('binds');
  box.textContent = '';
  for (const [name] of BUTTONS) {
    const b = document.createElement('button');
    b.dataset.n = name;
    b.classList.toggle('wait', listening === name);
    const label = document.createElement('span');
    label.textContent = name;
    const key = document.createElement('i');
    key.textContent = listening === name ? 'нажми клавишу…' : keyLabel(binds[name]);
    b.append(label, key);
    b.onclick = () => { listening = name; renderBinds(); };
    btnEls[name] = b;
    box.append(b);
  }
}

document.getElementById('reset').onclick = () => {
  binds = { ...DEFAULTS }; listening = null; saveBinds(); renderBinds();
};

addEventListener('keydown', e => {
  if (listening) {
    e.preventDefault();
    if (e.code !== 'Escape') {
      // если клавиша уже занята — меняем местами
      for (const n in binds) if (binds[n] === e.code) binds[n] = binds[listening];
      binds[listening] = e.code;
      saveBinds();
    }
    listening = null;
    renderBinds();
    return;
  }
  if (audioCtx && audioCtx.state === 'suspended') audioCtx.resume();
  down.add(e.code);
  if (Object.values(binds).includes(e.code)) e.preventDefault();
});
addEventListener('keyup', e => down.delete(e.code));
addEventListener('blur', () => down.clear());

// кнопка стандартного геймпада → бит маски JOY1 (нижняя A → SNES B, как на Xbox/SNES-раскладке)
const PAD = [
  [0, 15], [1, 7], [2, 14], [3, 6], [4, 5], [5, 4], [8, 13], [9, 12],
  [12, 11], [13, 10], [14, 9], [15, 8],
];
function gamepadMask() {
  let m = 0;
  for (const gp of navigator.getGamepads?.() ?? []) {
    if (!gp) continue;
    for (const [i, bit] of PAD) if (gp.buttons[i]?.pressed) m |= 1 << bit;
    const x = gp.axes[0] ?? 0, y = gp.axes[1] ?? 0;
    if (x < -0.5) m |= 1 << 9;
    if (x > 0.5) m |= 1 << 8;
    if (y < -0.5) m |= 1 << 11;
    if (y > 0.5) m |= 1 << 10;
  }
  return m;
}
function padMask() {
  let m = gamepadMask();
  for (const [name, bit] of BUTTONS) if (down.has(binds[name])) m |= 1 << bit;
  return m;
}

const view = document.getElementById('screen');
const vctx = view.getContext('2d');
const src = document.createElement('canvas'); // «сырой» кадр 256×224
src.width = W; src.height = H;
const sctx = src.getContext('2d');
let hasFrame = false;
function redraw() {
  if (!hasFrame) return;
  vctx.imageSmoothingEnabled = false;
  vctx.drawImage(src, 0, 0, view.width, view.height);
}
// Целочисленный масштаб в физических пикселях экрана: при нецелом масштабе колонки пикселей
// получаются разной ширины и при горизонтальном скролле картинка «дрожит».
function fit() {
  const dpr = window.devicePixelRatio || 1;
  const avail = Math.min(document.documentElement.clientWidth - 32, 1024);
  const scale = Math.max(1, Math.floor((avail * dpr) / W));
  view.width = W * scale;
  view.height = H * scale;
  view.style.width = `${(W * scale) / dpr}px`;
  view.style.height = `${(H * scale) / dpr}px`;
  redraw();
}
addEventListener('resize', fit);
fit();
const status = document.getElementById('status');
const dbg = document.getElementById('dbg');

// ---- звук ----
let audioCtx = null, audioNode = null, muted = false;
async function startAudio() {
  if (audioCtx) { if (audioCtx.state === 'suspended') audioCtx.resume(); return; }
  try {
    audioCtx = new AudioContext({ latencyHint: 'interactive' });
    await audioCtx.audioWorklet.addModule('audio-worklet.js');
    audioNode = new AudioWorkletNode(audioCtx, 'snes-audio', { outputChannelCount: [2] });
    audioNode.connect(audioCtx.destination);
    if (audioCtx.state === 'suspended') await audioCtx.resume();
  } catch (err) {
    console.warn('Звук недоступен:', err);
    audioCtx = null; audioNode = null;
  }
}
function sendAudio() {
  const pcm = emu.take_audio(); // забираем всегда, чтобы буфер в wasm не рос
  if (audioNode && !muted && pcm.length) audioNode.port.postMessage(pcm, [pcm.buffer]);
}
const muteBtn = document.getElementById('mute');
muteBtn.onclick = () => {
  muted = !muted;
  muteBtn.textContent = muted ? 'Звук: выкл' : 'Звук: вкл';
  startAudio();
};
document.getElementById('rom').addEventListener('click', startAudio);
let wasm, emu = null, last = 0, acc = 0, paused = false;
let romBytes = null, romName = '', romKey = null, sramTick = 0;

// ---- сохранения (SRAM картриджа) в localStorage ----
function fnv(u8) {
  let h = 0x811c9dc5;
  for (let i = 0; i < u8.length; i++) { h ^= u8[i]; h = Math.imul(h, 0x01000193); }
  return (h >>> 0).toString(16);
}
function toB64(u8) {
  let s = '';
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
  return btoa(s);
}
function fromB64(s) {
  const b = atob(s), u = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i);
  return u;
}
function saveSram(force) {
  if (!emu || !romKey || emu.sram_len() === 0) return;
  const dirty = emu.take_sram_dirty();
  if (!dirty && !force) return;
  try { localStorage.setItem(romKey, toB64(emu.sram())); }
  catch (err) { console.warn('Не удалось сохранить SRAM:', err); }
}
addEventListener('pagehide', () => saveSram(true));
document.addEventListener('visibilitychange', () => { if (document.hidden) saveSram(true); });

function loadRom(bytes, name) {
  try {
    const next = new Emulator(bytes);
    saveSram(true);
    emu?.free();
    emu = next;
    romBytes = bytes; romName = name;
    romKey = 'snes-sram:' + fnv(bytes) + ':' + bytes.length;
    let note = '';
    if (emu.sram_len() > 0) {
      note = ' · сохранения';
      try {
        const saved = localStorage.getItem(romKey);
        if (saved) emu.load_sram(fromB64(saved));
      } catch (err) { console.warn('Не удалось прочитать SRAM:', err); }
    }
    status.textContent = `${emu.title() || name} (${emu.mapping()})${note}`;
  } catch (err) {
    status.textContent = `Ошибка: ${err}`;
  }
}

document.getElementById('rom').onchange = async e => {
  const file = e.target.files[0];
  if (file) loadRom(new Uint8Array(await file.arrayBuffer()), file.name);
};
const pauseBtn = document.getElementById('pause');
pauseBtn.onclick = () => {
  paused = !paused;
  pauseBtn.textContent = paused ? 'Продолжить' : 'Пауза';
};
document.getElementById('restart').onclick = () => { if (romBytes) loadRom(romBytes, romName); };

let dtAvg = 1000 / 60, tick = 0;
function frame(t) {
  requestAnimationFrame(frame);
  const dt = t - last; last = t;
  if (dt > 2 && dt < 100) dtAvg += (dt - dtAvg) * 0.05; // сглаженный период обновления экрана
  for (const [name] of BUTTONS) btnEls[name]?.classList.toggle('on', down.has(binds[name]));
  if (!emu || paused) { acc = 0; return; }
  emu.set_joypad(padMask());
  // Если частота экрана кратна ~60 Гц (60/120/180/240), привязываемся к ней: ровно один кадр
  // эмулятора на N кадров экрана. Иначе — накопитель времени (возможны небольшие рывки).
  const hz = 1000 / dtAvg;
  const n = Math.max(1, Math.round(hz / 60.0988));
  const locked = Math.abs(hz / n - 60.0988) < 2.5;
  let frames = 0;
  if (locked) {
    acc = 0;
    if (++tick >= n) { tick = 0; frames = 1; }
  } else {
    tick = 0;
    acc = Math.min(acc + dt, STEP * 3);
    while (acc >= STEP) { acc -= STEP; frames++; }
  }
  for (let i = 0; i < frames; i++) emu.run_frame();
  const ran = frames > 0;
  if (ran) {
    sendAudio();
    if (++sramTick % 120 === 0) saveSram(false); // раз в ~2 с, если SRAM менялась
    dbg.textContent = emu.cpu_state() + '\n' + emu.spc_state();
    const px = new Uint8ClampedArray(wasm.memory.buffer, emu.framebuffer_ptr(), W * H * 4);
    sctx.putImageData(new ImageData(px, W, H), 0, 0);
    hasFrame = true;
    redraw();
  }
}

renderBinds();
wasm = await init();
requestAnimationFrame(frame);
