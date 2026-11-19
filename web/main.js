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

function padMask() {
  let m = 0;
  for (const [name, bit] of BUTTONS) if (down.has(binds[name])) m |= 1 << bit;
  return m;
}

const ctx = document.getElementById('screen').getContext('2d');
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
let wasm, emu = null, last = 0, acc = 0;

document.getElementById('rom').onchange = async e => {
  const file = e.target.files[0];
  if (!file) return;
  try {
    const next = new Emulator(new Uint8Array(await file.arrayBuffer()));
    emu?.free();
    emu = next;
    status.textContent = `${emu.title() || file.name} (${emu.mapping()})`;
  } catch (err) {
    status.textContent = `Ошибка: ${err}`;
  }
};

function frame(t) {
  requestAnimationFrame(frame);
  const dt = t - last; last = t;
  for (const [name] of BUTTONS) btnEls[name]?.classList.toggle('on', down.has(binds[name]));
  if (!emu) { acc = 0; return; }
  acc = Math.min(acc + dt, STEP * 3);
  emu.set_joypad(padMask());
  let ran = false;
  while (acc >= STEP) { emu.run_frame(); acc -= STEP; ran = true; }
  if (ran) {
    sendAudio();
    dbg.textContent = emu.cpu_state() + '\n' + emu.spc_state();
    const px = new Uint8ClampedArray(wasm.memory.buffer, emu.framebuffer_ptr(), W * H * 4);
    ctx.putImageData(new ImageData(px, W, H), 0, 0);
  }
}

renderBinds();
wasm = await init();
requestAnimationFrame(frame);
