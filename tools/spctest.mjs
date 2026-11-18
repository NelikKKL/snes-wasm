// Прогон spctest.sfc (gilyon/snes-tests) через собранный wasm.
// Использование: node tools/spctest.mjs tests/data/spctest.sfc tests/data/spctest.txt
import fs from 'node:fs';

const { initSync, Emulator } = await import(new URL('../web/pkg/snes_web.js', import.meta.url).href);
const [, , romPath, descPath, maxFramesArg] = process.argv;
const wasm = initSync({ module: fs.readFileSync(new URL('../web/pkg/snes_web_bg.wasm', import.meta.url)) });
const emu = new Emulator(new Uint8Array(fs.readFileSync(romPath)));
const font = fs.readFileSync(new URL('../tests/data/font.bin', import.meta.url));

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

const maxFrames = parseInt(maxFramesArg || '6000');
let result = 'timeout', lastNum = '????';
for (let f = 0; f < maxFrames; f++) {
  emu.run_frame();
  const fb = new Uint8Array(wasm.memory.buffer, emu.framebuffer_ptr(), 256 * 224 * 4);
  const status = ocr(fb, 1, 18, 7);
  lastNum = ocr(fb, 3, 14, 4);
  if (status.startsWith('Success')) { result = 'success'; break; }
  if (status.startsWith('Failed')) {
    result = 'failed';
    console.log(`\nУпал тест #${lastNum}: PSW=${ocr(fb, 8, 5, 2)} A=${ocr(fb, 5, 5, 2)} X=${ocr(fb, 6, 5, 2)} Y=${ocr(fb, 7, 5, 2)}`);
    console.log(desc[lastNum.toUpperCase()] ?? '(нет описания)');
    break;
  }
  if (f % 500 === 499) console.log(`кадр ${f + 1}: тест ${lastNum}  ${emu.cpu_state()} | ${emu.spc_state()}`);
}
if (result === 'timeout') console.log(`ТАЙМАУТ: последний тест ${lastNum}\n${emu.cpu_state()}\n${emu.spc_state()}`);
console.log(`${romPath}: ${result}`);
process.exit(result === 'success' ? 0 : 1);
