# Генерирует крошечные ROM для проверки DSP: загружают программу в SPC700 через IPL и играют синус.
# Запуск из корня репозитория: python3 tools/mkaudiorom.py
import sys, math, struct

def build(dsp_regs, path):
    # ---- BRR: один период синуса в 16 отсчётов, range 11, filter 0, loop+end ----
    nibs = [round(7 * math.sin(2 * math.pi * k / 16)) & 0xF for k in range(16)]
    brr = bytes([0xB3] + [(nibs[2*i] << 4) | nibs[2*i+1] for i in range(8)])
    # ---- программа SPC700: записать регистры DSP и зациклиться ----
    code = bytearray()
    for reg, val in dsp_regs:
        code += bytes([0x8F, reg, 0xF2, 0x8F, val, 0xF3])   # mov $f2,#reg ; mov $f3,#val
    code += bytes([0x2F, 0xFE])                              # bra $
    payload = bytearray(0x20)
    payload[0:4] = bytes([0x10, 0x02, 0x10, 0x02])           # DIR[0] = {start $0210, loop $0210}
    payload[0x10:0x19] = brr
    payload += code
    n = len(payload)
    assert n < 125

    # ---- S-CPU: загрузка через IPL ----
    rom = bytearray(0x8000)
    c = bytearray(); labels = {}; fix = []
    def emit(*b): c.extend(b)
    def label(name): labels[name] = len(c)
    def br(op, name): emit(op, 0); fix.append((len(c) - 1, name))
    emit(0x78)                                               # SEI
    label('a'); emit(0xAD, 0x40, 0x21, 0xC9, 0xAA); br(0xD0, 'a')   # ждём $AA в порту 0
    label('b'); emit(0xAD, 0x41, 0x21, 0xC9, 0xBB); br(0xD0, 'b')   # ждём $BB в порту 1
    emit(0xA9, 0x00, 0x8D, 0x42, 0x21)                       # адрес назначения $0200
    emit(0xA9, 0x02, 0x8D, 0x43, 0x21)
    emit(0xA9, 0x01, 0x8D, 0x41, 0x21)                       # порт1 != 0: передача данных
    emit(0xA9, 0xCC, 0x8D, 0x40, 0x21)
    label('c'); emit(0xAD, 0x40, 0x21, 0xC9, 0xCC); br(0xD0, 'c')
    emit(0xA2, 0x00)                                         # LDX #0
    label('d'); emit(0xBD, 0x00, 0x81, 0x8D, 0x41, 0x21, 0x8A, 0x8D, 0x40, 0x21)
    label('e'); emit(0xCD, 0x40, 0x21); br(0xD0, 'e')
    emit(0xE8, 0xE0, n); br(0xD0, 'd')
    emit(0xA9, 0x20, 0x8D, 0x42, 0x21)                       # точка входа $0220
    emit(0xA9, 0x02, 0x8D, 0x43, 0x21)
    emit(0xA9, 0x00, 0x8D, 0x41, 0x21)                       # порт1 = 0: запустить
    emit(0xE8, 0xE8, 0x8A, 0x8D, 0x40, 0x21)                 # счётчик = N+2
    label('f'); emit(0xCD, 0x40, 0x21); br(0xD0, 'f')
    label('g'); emit(0x80, 0xFE)
    for pos, name in fix:
        rel = labels[name] - (pos + 1)
        assert -128 <= rel < 128
        c[pos] = rel & 0xFF
    assert len(c) < 0x100
    rom[0:len(c)] = c
    rom[0x100:0x100 + n] = payload
    h = 0x7FC0
    rom[h:h+21] = b'AUDIO TEST'.ljust(21)
    rom[h+0x15] = 0x20; rom[h+0x17] = 0x05
    rom[h+0x1C:h+0x1E] = b'\xFF\xFF'
    rom[0x7FFC:0x7FFE] = b'\x00\x80'
    open(path, 'wb').write(rom)

# сценарий A: тон, огибающая GAIN прямая
base = [(0x6C,0x20),(0x0C,0x7F),(0x1C,0x7F),(0x2C,0),(0x3C,0),(0x5D,0x02),
        (0x00,0x7F),(0x01,0x7F),(0x02,0x00),(0x03,0x10),(0x04,0x00)]
build(base + [(0x05,0x00),(0x07,0x7F),(0x4C,0x01)], 'tests/data/audio-tone.sfc')
# сценарий B: ADSR, медленная атака (rate 9), sustain level 7
build(base + [(0x05,0x84),(0x06,0xE0),(0x4C,0x01)], 'tests/data/audio-adsr.sfc')
# сценарий C: тон с питчем x2 (4 кГц)
build([(r,v) if r != 0x03 else (r,0x20) for r,v in base] + [(0x05,0x00),(0x07,0x7F),(0x4C,0x01)], 'tests/data/audio-tone2x.sfc')
print('ok')
