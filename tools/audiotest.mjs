// Сквозная проверка звука на собранном wasm: S-CPU → IPL → программа SPC700 → DSP → отсчёты.
import fs from 'node:fs';

const { initSync, Emulator } = await import(new URL('../web/pkg/snes_web.js', import.meta.url).href);
initSync({ module: fs.readFileSync(new URL('../web/pkg/snes_web_bg.wasm', import.meta.url)) });

function capture(file, frames = 120) {
  const rom = new Uint8Array(fs.readFileSync(new URL(`../tests/data/${file}`, import.meta.url)));
  const emu = new Emulator(rom);
  const left = [];
  for (let f = 0; f < frames; f++) {
    emu.run_frame();
    const pcm = emu.take_audio();
    for (let i = 0; i < pcm.length; i += 2) left.push(pcm[i]);
  }
  return left;
}
const peak = (a) => a.reduce((m, v) => Math.max(m, Math.abs(v)), 0);
function freq(seg) {
  let c = 0;
  for (let i = 1; i < seg.length; i++) if ((seg[i - 1] >= 0) !== (seg[i] >= 0)) c++;
  return c / 2 / (seg.length / 32000);
}

const results = [];
function check(name, ok, info) {
  results.push(ok);
  console.log(`${ok ? 'OK  ' : 'FAIL'} ${name}: ${info}`);
}

for (const [file, hz] of [['audio-tone.sfc', 2000], ['audio-tone2x.sfc', 4000]]) {
  const left = capture(file);
  const first = left.findIndex((v) => v !== 0);
  const seg = first < 0 ? [] : left.slice(first + 3200, first + 3200 + 16000);
  check(`${file}: звук пошёл`, seg.length === 16000, `первый ненулевой отсчёт ${first}`);
  if (seg.length) {
    const f = freq(seg), p = peak(seg);
    check(`${file}: частота`, Math.abs(f - hz) < hz * 0.02, `${f.toFixed(0)} Гц (ждали ${hz})`);
    check(`${file}: амплитуда`, p > 12000 && p < 15000, `пик ${p}`);
  }
}

{
  const left = capture('audio-adsr.sfc');
  const first = left.findIndex((v) => v !== 0);
  const peaks = [];
  for (let i = first; first >= 0 && i + 3200 <= left.length; i += 3200) peaks.push(peak(left.slice(i, i + 3200)));
  const rising = peaks.slice(1, 7).every((p, i) => p > peaks[i]);
  check('ADSR: атака нарастает', first >= 0 && peaks[0] < 4000 && rising, `окна по 0.1 с: ${peaks.slice(0, 8).join(', ')}`);
  check('ADSR: sustain на максимуме', peaks.length > 0 && peak(left.slice(-3200)) > 12000, `пик последнего окна ${peaks.at(-1)}`);
}

process.exit(results.every(Boolean) ? 0 : 1);
