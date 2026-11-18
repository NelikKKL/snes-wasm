// Прогон тест-ROM gilyon/snes-tests через собранный wasm (web/pkg), без браузера.
// После каждого падения «нажимает A», так что в отчёт попадают все упавшие тесты.
// Использование: node tools/cputest.mjs tests/data/cputest-basic.sfc tests/data/cputest-basic.txt
import fs from 'node:fs';

const { initSync, Emulator } = await import(new URL('../web/pkg/snes_web.js', import.meta.url).href);
const [, , romPath, descPath, maxFramesArg] = process.argv;
const wasm = initSync({ module: fs.readFileSync(new URL('../web/pkg/snes_web_bg.wasm', import.meta.url)) });
const emu = new Emulator(new Uint8Array(fs.readFileSync(romPath)));
const font = fs.readFileSync(new URL('../tests/data/font.bin', import.meta.url));

// эталонные глифы 8x8 для ASCII — по ним читаем текст с экрана
const glyphs = {};
for (let ch = 0x20; ch < 0x7f; ch++) {
  let bits = '';
  for (let r = 0; r < 8; r++) {
    const v = font[ch * 16 + r * 2] | font[ch * 16 + r * 2 + 1];
    for (let x = 7; x >= 0; x--) bits += (v >> x) & 1;
  }
  glyphs[bits] = String.fromCharCode(ch);
}
function ocr(fb, row, col, len) {
  let s = '';
  for (let i = 0; i < len; i++) {
    let bits = '';
    for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) {
      bits += fb[((row * 8 + y) * 256 + col * 8 + x + i * 8) * 4] > 128 ? 1 : 0;
    }
    s += glyphs[bits] ?? '?';
  }
  return s;
}

const desc = {};
{
  let cur = null;
  for (const line of fs.readFileSync(descPath, 'utf8').split('\n')) {
    const m = line.match(/^Test ([0-9a-f]{4}):/i);
    if (m) { cur = m[1].toUpperCase(); desc[cur] = line; }
    else if (cur && line.startsWith('   ')) desc[cur] += '\n' + line;
  }
}

const maxFrames = parseInt(maxFramesArg || '12000');
const fails = new Map();
let done = false, pressing = 0;
for (let f = 0; f < maxFrames && !done; f++) {
  emu.run_frame();
  if (pressing > 0) { if (--pressing === 0) emu.set_joypad(0); continue; }
  const fb = new Uint8Array(wasm.memory.buffer, emu.framebuffer_ptr(), 256 * 224 * 4);
  const status = ocr(fb, 1, 18, 7);
  if (status.startsWith('Success')) done = true;
  else if (status.startsWith('Failed') || status.startsWith('Invalid')) {
    const num = ocr(fb, 3, 14, 4);
    if (!fails.has(num)) {
      fails.set(num, `got A=${ocr(fb, 5, 5, 4)} X=${ocr(fb, 6, 5, 4)} Y=${ocr(fb, 7, 5, 4)} P=${ocr(fb, 8, 5, 2)} S=${ocr(fb, 9, 5, 4)}`);
    }
    if (status.startsWith('Invalid')) break;
    emu.set_joypad(0x0080); pressing = 4;
  }
}
for (const [num, got] of fails) console.log(`\n#${num} ${got}\n${desc[num] ?? '(нет описания)'}`);
console.log(`\n${romPath}: ${done ? 'дошёл до Success' : 'НЕ ЗАВЕРШИЛСЯ'}, упавших тестов: ${fails.size}`);
process.exit(fails.size || !done ? 1 : 0);
