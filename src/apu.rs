//! SPC700 (звуковой процессор), память APU, порты связи с S-CPU и таймеры.
//! DSP пока только набор регистров — звук добавим следующим шагом.

/// Загрузочный ROM, виден в $FFC0–$FFFF пока бит 7 регистра $F1 установлен.
const IPL: [u8; 64] = [
    0xCD, 0xEF, 0xBD, 0xE8, 0x00, 0xC6, 0x1D, 0xD0, 0xFC, 0x8F, 0xAA, 0xF4, 0x8F, 0xBB, 0xF5, 0x78,
    0xCC, 0xF4, 0xD0, 0xFB, 0x2F, 0x19, 0xEB, 0xF4, 0xD0, 0xFC, 0x7E, 0xF4, 0xD0, 0x0B, 0xE4, 0xF5,
    0xCB, 0xF4, 0xD7, 0x00, 0xFC, 0xD0, 0xF3, 0xAB, 0x01, 0x10, 0xEF, 0x7E, 0xF4, 0x10, 0xEB, 0xBA,
    0xF6, 0xDA, 0x00, 0xBA, 0xF4, 0xC4, 0xF4, 0xDD, 0x5D, 0xD0, 0xDB, 0x1F, 0x00, 0x00, 0xC0, 0xFF,
];

/// Такты на опкод (для условных переходов — без штрафа за «взят», он добавляется отдельно).
const CYCLES: [u8; 256] = [
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 6, 8,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 4, 6,
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 5, 4,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 3, 8,
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 6, 6,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 4, 5, 2, 2, 4, 3,
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 5, 5,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 6,
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 2, 4, 5,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 12, 5,
    3, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 2, 4, 4,
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 4,
    3, 8, 4, 5, 4, 5, 4, 7, 2, 5, 6, 4, 5, 2, 4, 9,
    2, 8, 4, 5, 5, 6, 6, 7, 4, 5, 5, 5, 2, 2, 6, 3,
    2, 8, 4, 5, 3, 4, 3, 6, 2, 4, 5, 3, 4, 3, 4, 3,
    2, 8, 4, 5, 4, 5, 5, 6, 3, 4, 5, 4, 2, 2, 4, 3,
];

const C: u8 = 0x01;
const Z: u8 = 0x02;
const I: u8 = 0x04;
const H: u8 = 0x08;
const B: u8 = 0x10;
const P: u8 = 0x20;
const V: u8 = 0x40;
const N: u8 = 0x80;

pub struct Apu {
    ram: Vec<u8>,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    pc: u16,
    psw: u8,
    halted: bool,
    ipl_on: bool,
    to_apu: [u8; 4],   // записано S-CPU, читается SPC700 в $F4–$F7
    from_apu: [u8; 4], // записано SPC700, читается S-CPU в $2140–$2143
    dsp_addr: u8,
    pub dsp: [u8; 128],
    t_en: [bool; 3],
    t_target: [u8; 3],
    t_acc: [u32; 3], // предделитель: 128, 128 и 16 тактов
    t_cnt: [u16; 3],
    t_out: [u8; 3],
    budget: i32,
    acc: u64,
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Apu {
            ram: vec![0; 0x10000], a: 0, x: 0, y: 0, sp: 0xEF, pc: 0, psw: 0,
            halted: false, ipl_on: true, to_apu: [0; 4], from_apu: [0; 4],
            dsp_addr: 0, dsp: [0; 128],
            t_en: [false; 3], t_target: [0; 3], t_acc: [0; 3], t_cnt: [0; 3], t_out: [0; 3],
            budget: 0, acc: 0,
        };
        apu.pc = apu.rd16(0xFFFE);
        apu
    }

    pub fn port_read(&self, i: usize) -> u8 { self.from_apu[i & 3] }
    pub fn port_write(&mut self, i: usize, v: u8) { self.to_apu[i & 3] = v; }

    pub fn debug(&self) -> String {
        format!(
            "SPC {:04X} A={:02X} X={:02X} Y={:02X} SP={:02X} PSW={:02X}",
            self.pc, self.a, self.x, self.y, self.sp, self.psw
        )
    }

    /// Прокрутить SPC700 на `master` master-тактов S-CPU (21.477 МГц → 1.024 МГц).
    pub fn clock(&mut self, master: u32) {
        self.acc += master as u64 * 1_024_000;
        self.budget += (self.acc / 21_477_272) as i32;
        self.acc %= 21_477_272;
        while self.budget > 0 {
            let c = self.step();
            self.budget -= c as i32;
        }
    }

    // ---------- память и регистры ввода-вывода ----------
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0xF0..=0xFF => self.read_io(addr as u8),
            0xFFC0..=0xFFFF if self.ipl_on => IPL[(addr - 0xFFC0) as usize],
            _ => self.ram[addr as usize],
        }
    }

    fn write(&mut self, addr: u16, v: u8) {
        self.ram[addr as usize] = v; // под регистрами и IPL всегда лежит RAM ($F8/$F9 — обычная RAM)
        if (0xF0..=0xFF).contains(&addr) { self.write_io(addr as u8, v); }
    }

    fn read_io(&mut self, r: u8) -> u8 {
        match r {
            0xF2 => self.dsp_addr,
            0xF3 => self.dsp[(self.dsp_addr & 0x7F) as usize],
            0xF4..=0xF7 => self.to_apu[(r - 0xF4) as usize],
            0xF8 | 0xF9 => self.ram[r as usize],
            0xFD..=0xFF => {
                let i = (r - 0xFD) as usize;
                let v = self.t_out[i];
                self.t_out[i] = 0; // счётчик сбрасывается при чтении
                v
            }
            _ => 0,
        }
    }

    fn write_io(&mut self, r: u8, v: u8) {
        match r {
            0xF1 => {
                for i in 0..3 {
                    let en = v & (1 << i) != 0;
                    if en && !self.t_en[i] { self.t_cnt[i] = 0; self.t_out[i] = 0; }
                    self.t_en[i] = en;
                }
                if v & 0x10 != 0 { self.to_apu[0] = 0; self.to_apu[1] = 0; }
                if v & 0x20 != 0 { self.to_apu[2] = 0; self.to_apu[3] = 0; }
                self.ipl_on = v & 0x80 != 0;
            }
            0xF2 => self.dsp_addr = v,
            0xF3 => { if self.dsp_addr < 0x80 { self.dsp[self.dsp_addr as usize] = v; } }
            0xF4..=0xF7 => self.from_apu[(r - 0xF4) as usize] = v,
            0xFA..=0xFC => self.t_target[(r - 0xFA) as usize] = v,
            _ => {}
        }
    }

    fn tick_timers(&mut self, cycles: u32) {
        const PERIOD: [u32; 3] = [128, 128, 16];
        for i in 0..3 {
            self.t_acc[i] += cycles;
            while self.t_acc[i] >= PERIOD[i] {
                self.t_acc[i] -= PERIOD[i];
                if self.t_en[i] {
                    self.t_cnt[i] += 1;
                    let target = if self.t_target[i] == 0 { 256 } else { self.t_target[i] as u16 };
                    if self.t_cnt[i] >= target {
                        self.t_cnt[i] = 0;
                        self.t_out[i] = (self.t_out[i] + 1) & 0x0F;
                    }
                }
            }
        }
    }

    // ---------- вспомогательные операции ----------
    fn rd16(&mut self, a: u16) -> u16 {
        let lo = self.read(a) as u16;
        lo | (self.read(a.wrapping_add(1)) as u16) << 8
    }
    fn fetch(&mut self) -> u8 {
        let v = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch() as u16;
        lo | (self.fetch() as u16) << 8
    }
    /// Адрес в direct page (страница 0 или 1 в зависимости от флага P).
    fn dpa(&self, off: u8) -> u16 { (((self.psw & P) as u16) << 3) | off as u16 }
    fn rd_dp(&mut self, off: u8) -> u8 { let a = self.dpa(off); self.read(a) }
    fn wr_dp(&mut self, off: u8, v: u8) { let a = self.dpa(off); self.write(a, v); }
    /// 16-битное слово в direct page; старший байт читается с переносом внутри страницы.
    fn rd_dpw(&mut self, off: u8) -> u16 {
        let lo = self.rd_dp(off) as u16;
        lo | (self.rd_dp(off.wrapping_add(1)) as u16) << 8
    }
    fn wr_dpw(&mut self, off: u8, v: u16) {
        self.wr_dp(off, v as u8);
        self.wr_dp(off.wrapping_add(1), (v >> 8) as u8);
    }
    fn push(&mut self, v: u8) {
        let a = 0x100 | self.sp as u16;
        self.write(a, v);
        self.sp = self.sp.wrapping_sub(1);
    }
    fn pop(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        let a = 0x100 | self.sp as u16;
        self.read(a)
    }
    fn push16(&mut self, v: u16) { self.push((v >> 8) as u8); self.push(v as u8); }
    fn pop16(&mut self) -> u16 {
        let lo = self.pop() as u16;
        lo | (self.pop() as u16) << 8
    }
    fn ya(&self) -> u16 { ((self.y as u16) << 8) | self.a as u16 }
    fn set(&mut self, f: u8, on: bool) { if on { self.psw |= f } else { self.psw &= !f } }
    fn nz(&mut self, v: u8) {
        self.psw = (self.psw & !(N | Z)) | (v & 0x80) | if v == 0 { Z } else { 0 };
    }
    fn nz16(&mut self, v: u16) {
        self.psw = (self.psw & !(N | Z)) | if v & 0x8000 != 0 { N } else { 0 } | if v == 0 { Z } else { 0 };
    }
    fn branch(&mut self, rel: i8) -> u32 {
        self.pc = self.pc.wrapping_add(rel as i16 as u16);
        2
    }

    fn cmp(&mut self, a: u8, b: u8) {
        self.set(C, a >= b);
        self.nz(a.wrapping_sub(b));
    }

    fn adc(&mut self, a: u8, b: u8) -> u8 {
        let c = self.psw & C;
        let sum = a as u16 + b as u16 + c as u16;
        let r = sum as u8;
        self.set(C, sum > 0xFF);
        self.set(H, (a & 0xF) + (b & 0xF) + c > 0xF);
        self.set(V, (!(a ^ b) & (a ^ r) & 0x80) != 0);
        self.nz(r);
        r
    }

    /// kind: 0 OR, 1 AND, 2 EOR, 3 CMP, 4 ADC, 5 SBC. Для CMP возвращает `a` без изменений.
    fn alu(&mut self, kind: u8, a: u8, b: u8) -> u8 {
        match kind {
            0 => { let r = a | b; self.nz(r); r }
            1 => { let r = a & b; self.nz(r); r }
            2 => { let r = a ^ b; self.nz(r); r }
            3 => { self.cmp(a, b); a }
            4 => self.adc(a, b),
            _ => self.adc(a, !b),
        }
    }

    /// kind: 0 ASL, 1 ROL, 2 LSR, 3 ROR, 4 DEC, 5 INC
    fn rot(&mut self, kind: u8, v: u8) -> u8 {
        let c = self.psw & C;
        let r = match kind {
            0 => { self.set(C, v & 0x80 != 0); v << 1 }
            1 => { self.set(C, v & 0x80 != 0); (v << 1) | c }
            2 => { self.set(C, v & 1 != 0); v >> 1 }
            3 => { self.set(C, v & 1 != 0); (v >> 1) | (c << 7) }
            4 => v.wrapping_sub(1),
            _ => v.wrapping_add(1),
        };
        self.nz(r);
        r
    }

    fn rmw(&mut self, kind: u8, ea: u16) {
        let v = self.read(ea);
        let r = self.rot(kind, v);
        self.write(ea, r);
    }

    /// Эффективный адрес для регулярных режимов (младшие 5 бит опкода).
    fn ea(&mut self, m: u8) -> u16 {
        match m {
            0x04 => { let o = self.fetch(); self.dpa(o) }
            0x05 => self.fetch16(),
            0x06 => self.dpa(self.x),
            0x07 => { let o = self.fetch().wrapping_add(self.x); let p = self.rd_dpw(o); p }
            0x08 => { let a = self.pc; self.pc = self.pc.wrapping_add(1); a }
            0x14 => { let o = self.fetch().wrapping_add(self.x); self.dpa(o) }
            0x15 => self.fetch16().wrapping_add(self.x as u16),
            0x16 => self.fetch16().wrapping_add(self.y as u16),
            _ => { let o = self.fetch(); self.rd_dpw(o).wrapping_add(self.y as u16) } // 0x17: [dp]+Y
        }
    }

    // ---------- выполнение ----------
    pub fn step(&mut self) -> u32 {
        if self.halted { return 2; }
        let op = self.fetch();
        let cyc = CYCLES[op as usize] as u32 + self.exec(op);
        self.tick_timers(cyc);
        cyc
    }

    /// Возвращает дополнительные такты (взятые переходы).
    fn exec(&mut self, op: u8) -> u32 {
        // OR AND EOR CMP ADC SBC во всех режимах
        if op < 0xC0 && matches!(op & 0x0F, 4..=9) {
            let kind = op >> 5;
            match op & 0x1F {
                0x09 => {
                    let (s, d) = (self.fetch(), self.fetch());
                    let (sv, dv) = (self.rd_dp(s), self.rd_dp(d));
                    let r = self.alu(kind, dv, sv);
                    if kind != 3 { self.wr_dp(d, r); }
                }
                0x18 => {
                    let (imm, d) = (self.fetch(), self.fetch());
                    let dv = self.rd_dp(d);
                    let r = self.alu(kind, dv, imm);
                    if kind != 3 { self.wr_dp(d, r); }
                }
                0x19 => {
                    let (xa, ya) = (self.dpa(self.x), self.dpa(self.y));
                    let (xv, yv) = (self.read(xa), self.read(ya));
                    let r = self.alu(kind, xv, yv);
                    if kind != 3 { self.write(xa, r); }
                }
                m => {
                    let ea = self.ea(m);
                    let b = self.read(ea);
                    let a = self.a;
                    let r = self.alu(kind, a, b);
                    if kind != 3 { self.a = r; }
                }
            }
            return 0;
        }
        // SET1/CLR1/BBS/BBC/Bcc/TCALL — закономерности в младших битах
        match op & 0x1F {
            0x02 | 0x12 => {
                let bit = op >> 5;
                let o = self.fetch();
                let a = self.dpa(o);
                let v = self.read(a);
                self.write(a, if op & 0x10 == 0 { v | (1 << bit) } else { v & !(1 << bit) });
                return 0;
            }
            0x03 | 0x13 => {
                let bit = op >> 5;
                let o = self.fetch();
                let rel = self.fetch() as i8;
                let set = self.rd_dp(o) & (1 << bit) != 0;
                return if set == (op & 0x10 == 0) { self.branch(rel) } else { 0 };
            }
            0x10 => {
                let rel = self.fetch() as i8;
                let idx = op >> 5;
                let flag = [N, V, C, Z][(idx >> 1) as usize];
                return if (self.psw & flag != 0) == (idx & 1 == 1) { self.branch(rel) } else { 0 };
            }
            _ => {}
        }
        if op & 0x0F == 1 {
            // TCALL n
            let n = (op >> 4) as u16;
            let pc = self.pc;
            self.push16(pc);
            self.pc = self.rd16(0xFFDE - n * 2);
            return 0;
        }

        match op {
            0x00 => 0,
            0xEF | 0xFF => { self.halted = true; 0 } // SLEEP / STOP

            // ----- MOV -----
            0xE4..=0xE8 | 0xF4..=0xF7 => {
                let ea = self.ea(op & 0x1F);
                let v = self.read(ea);
                self.a = v;
                self.nz(v);
                0
            }
            0xC4..=0xC7 | 0xD4..=0xD7 => {
                let ea = self.ea(op & 0x1F);
                let a = self.a;
                self.write(ea, a);
                0
            }
            0xCD => { let v = self.fetch(); self.x = v; self.nz(v); 0 }
            0xF8 => { let o = self.fetch(); let v = self.rd_dp(o); self.x = v; self.nz(v); 0 }
            0xF9 => { let o = self.fetch().wrapping_add(self.y); let v = self.rd_dp(o); self.x = v; self.nz(v); 0 }
            0xE9 => { let a = self.fetch16(); let v = self.read(a); self.x = v; self.nz(v); 0 }
            0x8D => { let v = self.fetch(); self.y = v; self.nz(v); 0 }
            0xEB => { let o = self.fetch(); let v = self.rd_dp(o); self.y = v; self.nz(v); 0 }
            0xFB => { let o = self.fetch().wrapping_add(self.x); let v = self.rd_dp(o); self.y = v; self.nz(v); 0 }
            0xEC => { let a = self.fetch16(); let v = self.read(a); self.y = v; self.nz(v); 0 }
            0xD8 => { let o = self.fetch(); let v = self.x; self.wr_dp(o, v); 0 }
            0xD9 => { let o = self.fetch().wrapping_add(self.y); let v = self.x; self.wr_dp(o, v); 0 }
            0xC9 => { let a = self.fetch16(); let v = self.x; self.write(a, v); 0 }
            0xCB => { let o = self.fetch(); let v = self.y; self.wr_dp(o, v); 0 }
            0xDB => { let o = self.fetch().wrapping_add(self.x); let v = self.y; self.wr_dp(o, v); 0 }
            0xCC => { let a = self.fetch16(); let v = self.y; self.write(a, v); 0 }
            0x8F => { let imm = self.fetch(); let o = self.fetch(); self.wr_dp(o, imm); 0 }
            0xFA => { let s = self.fetch(); let d = self.fetch(); let v = self.rd_dp(s); self.wr_dp(d, v); 0 }
            0xBF => {
                let a = self.dpa(self.x);
                let v = self.read(a);
                self.x = self.x.wrapping_add(1);
                self.a = v;
                self.nz(v);
                0
            }
            0xAF => {
                let a = self.dpa(self.x);
                let v = self.a;
                self.write(a, v);
                self.x = self.x.wrapping_add(1);
                0
            }
            0x7D => { let v = self.x; self.a = v; self.nz(v); 0 }
            0xDD => { let v = self.y; self.a = v; self.nz(v); 0 }
            0x5D => { let v = self.a; self.x = v; self.nz(v); 0 }
            0xFD => { let v = self.a; self.y = v; self.nz(v); 0 }
            0x9D => { let v = self.sp; self.x = v; self.nz(v); 0 }
            0xBD => { self.sp = self.x; 0 }

            // ----- CMP X / CMP Y -----
            0xC8 | 0x3E | 0x1E => {
                let m = match op { 0xC8 => 0x08, 0x3E => 0x04, _ => 0x05 };
                let ea = self.ea(m);
                let v = self.read(ea);
                let x = self.x;
                self.cmp(x, v);
                0
            }
            0xAD | 0x7E | 0x5E => {
                let m = match op { 0xAD => 0x08, 0x7E => 0x04, _ => 0x05 };
                let ea = self.ea(m);
                let v = self.read(ea);
                let y = self.y;
                self.cmp(y, v);
                0
            }

            // ----- ASL ROL LSR ROR DEC INC -----
            0x0B | 0x2B | 0x4B | 0x6B | 0x8B | 0xAB => { let o = self.fetch(); let ea = self.dpa(o); self.rmw(op >> 5, ea); 0 }
            0x0C | 0x2C | 0x4C | 0x6C | 0x8C | 0xAC => { let ea = self.fetch16(); self.rmw(op >> 5, ea); 0 }
            0x1B | 0x3B | 0x5B | 0x7B | 0x9B | 0xBB => {
                let o = self.fetch().wrapping_add(self.x);
                let ea = self.dpa(o);
                self.rmw(op >> 5, ea);
                0
            }
            0x1C | 0x3C | 0x5C | 0x7C | 0x9C | 0xBC => { let v = self.a; self.a = self.rot(op >> 5, v); 0 }
            0x1D => { let v = self.x.wrapping_sub(1); self.x = v; self.nz(v); 0 }
            0x3D => { let v = self.x.wrapping_add(1); self.x = v; self.nz(v); 0 }
            0xDC => { let v = self.y.wrapping_sub(1); self.y = v; self.nz(v); 0 }
            0xFC => { let v = self.y.wrapping_add(1); self.y = v; self.nz(v); 0 }

            // ----- 16-битные -----
            0xBA => { let o = self.fetch(); let w = self.rd_dpw(o); self.a = w as u8; self.y = (w >> 8) as u8; self.nz16(w); 0 }
            0xDA => { let o = self.fetch(); let w = self.ya(); self.wr_dpw(o, w); 0 }
            0x1A | 0x3A => {
                let o = self.fetch();
                let w = self.rd_dpw(o);
                let r = if op == 0x3A { w.wrapping_add(1) } else { w.wrapping_sub(1) };
                self.wr_dpw(o, r);
                self.nz16(r);
                0
            }
            0x5A => {
                let o = self.fetch();
                let m = self.rd_dpw(o);
                let ya = self.ya();
                self.set(C, ya >= m);
                self.nz16(ya.wrapping_sub(m));
                0
            }
            0x7A => {
                let o = self.fetch();
                let m = self.rd_dpw(o) as u32;
                let ya = self.ya() as u32;
                let sum = ya + m;
                let r = sum as u16;
                self.set(C, sum > 0xFFFF);
                self.set(H, (ya & 0xFFF) + (m & 0xFFF) > 0xFFF);
                self.set(V, (!(ya ^ m) & (ya ^ r as u32) & 0x8000) != 0);
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                self.nz16(r);
                0
            }
            0x9A => {
                let o = self.fetch();
                let m = self.rd_dpw(o);
                let ya = self.ya();
                let r = ya.wrapping_sub(m);
                self.set(C, ya >= m);
                self.set(H, (ya & 0xFFF) >= (m & 0xFFF));
                self.set(V, ((ya ^ m) & (ya ^ r) & 0x8000) != 0);
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                self.nz16(r);
                0
            }
            0xCF => {
                let r = self.y as u16 * self.a as u16;
                self.a = r as u8;
                self.y = (r >> 8) as u8;
                let y = self.y;
                self.nz(y);
                0
            }
            0x9E => {
                let ya = self.ya() as u32;
                let (x, y) = (self.x as u32, self.y as u32);
                self.set(H, (y & 15) >= (x & 15));
                self.set(V, y >= x);
                if y < (x << 1) {
                    self.a = (ya / x) as u8;
                    self.y = (ya % x) as u8;
                } else {
                    let t = ya - (x << 9);
                    self.a = 255u32.wrapping_sub(t / (256 - x)) as u8;
                    self.y = (x + t % (256 - x)) as u8;
                }
                let a = self.a;
                self.nz(a);
                0
            }
            0xDF => {
                if self.psw & C != 0 || self.a > 0x99 { self.a = self.a.wrapping_add(0x60); self.psw |= C; }
                if self.psw & H != 0 || (self.a & 0x0F) > 9 { self.a = self.a.wrapping_add(6); }
                let a = self.a;
                self.nz(a);
                0
            }
            0xBE => {
                if self.psw & C == 0 || self.a > 0x99 { self.a = self.a.wrapping_sub(0x60); self.psw &= !C; }
                if self.psw & H == 0 || (self.a & 0x0F) > 9 { self.a = self.a.wrapping_sub(6); }
                let a = self.a;
                self.nz(a);
                0
            }
            0x9F => { self.a = self.a.rotate_left(4); let a = self.a; self.nz(a); 0 }

            // ----- биты -----
            0x0A | 0x2A | 0x4A | 0x6A | 0x8A | 0xAA | 0xCA | 0xEA => {
                let w = self.fetch16();
                let (addr, bit) = (w & 0x1FFF, (w >> 13) as u8);
                let v = self.read(addr);
                let b = (v >> bit) & 1 != 0;
                let c = self.psw & C != 0;
                match op {
                    0x0A => self.set(C, c | b),
                    0x2A => self.set(C, c | !b),
                    0x4A => self.set(C, c & b),
                    0x6A => self.set(C, c & !b),
                    0x8A => self.set(C, c ^ b),
                    0xAA => self.set(C, b),
                    0xCA => self.write(addr, (v & !(1 << bit)) | ((c as u8) << bit)),
                    _ => self.write(addr, v ^ (1 << bit)), // NOT1
                }
                0
            }
            0x0E | 0x4E => {
                let ea = self.fetch16();
                let v = self.read(ea);
                let a = self.a;
                self.nz(a.wrapping_sub(v));
                self.write(ea, if op == 0x0E { v | a } else { v & !a });
                0
            }

            // ----- стек, флаги -----
            0x0D => { let v = self.psw; self.push(v); 0 }
            0x2D => { let v = self.a; self.push(v); 0 }
            0x4D => { let v = self.x; self.push(v); 0 }
            0x6D => { let v = self.y; self.push(v); 0 }
            0x8E => { self.psw = self.pop(); 0 }
            0xAE => { self.a = self.pop(); 0 }
            0xCE => { self.x = self.pop(); 0 }
            0xEE => { self.y = self.pop(); 0 }
            0x20 => { self.psw &= !P; 0 }
            0x40 => { self.psw |= P; 0 }
            0x60 => { self.psw &= !C; 0 }
            0x80 => { self.psw |= C; 0 }
            0xA0 => { self.psw |= I; 0 }
            0xC0 => { self.psw &= !I; 0 }
            0xE0 => { self.psw &= !(V | H); 0 }
            0xED => { self.psw ^= C; 0 }

            // ----- переходы -----
            0x2F => { let rel = self.fetch() as i8; self.branch(rel) }
            0x5F => { self.pc = self.fetch16(); 0 }
            0x1F => {
                let t = self.fetch16().wrapping_add(self.x as u16);
                self.pc = self.rd16(t);
                0
            }
            0x3F => { let t = self.fetch16(); let pc = self.pc; self.push16(pc); self.pc = t; 0 }
            0x4F => { let u = self.fetch(); let pc = self.pc; self.push16(pc); self.pc = 0xFF00 | u as u16; 0 }
            0x6F => { self.pc = self.pop16(); 0 }
            0x7F => { self.psw = self.pop(); self.pc = self.pop16(); 0 }
            0x0F => {
                let pc = self.pc;
                self.push16(pc);
                let p = self.psw;
                self.push(p);
                self.psw = (self.psw | B) & !I;
                self.pc = self.rd16(0xFFDE);
                0
            }
            0x2E | 0xDE => {
                let o = if op == 0xDE { self.fetch().wrapping_add(self.x) } else { self.fetch() };
                let rel = self.fetch() as i8;
                let v = self.rd_dp(o);
                if self.a != v { self.branch(rel) } else { 0 }
            }
            0x6E => {
                let o = self.fetch();
                let rel = self.fetch() as i8;
                let v = self.rd_dp(o).wrapping_sub(1);
                self.wr_dp(o, v);
                if v != 0 { self.branch(rel) } else { 0 }
            }
            0xFE => {
                let rel = self.fetch() as i8;
                self.y = self.y.wrapping_sub(1);
                if self.y != 0 { self.branch(rel) } else { 0 }
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Кладёт программу в RAM по $0200, отключает IPL и выполняет `steps` инструкций.
    fn run(prog: &[u8], steps: usize) -> Apu {
        let mut a = Apu::new();
        a.ipl_on = false;
        a.ram[0x200..0x200 + prog.len()].copy_from_slice(prog);
        a.pc = 0x200;
        for _ in 0..steps { a.step(); }
        a
    }

    #[test]
    fn ipl_announces_aa_bb() {
        let mut a = Apu::new();
        for _ in 0..3000 { a.step(); }
        assert_eq!((a.port_read(0), a.port_read(1)), (0xAA, 0xBB));
    }

    #[test]
    fn adc_immediate() {
        // mov a,#$12; adc a,#$34
        let a = run(&[0xE8, 0x12, 0x88, 0x34], 2);
        assert_eq!(a.a, 0x46);
    }

    #[test]
    fn div_ya_x() {
        // mov y,#0; mov a,#100; mov x,#7; div ya,x
        let a = run(&[0x8D, 0x00, 0xE8, 0x64, 0xCD, 0x07, 0x9E], 4);
        assert_eq!((a.a, a.y), (14, 2));
    }

    #[test]
    fn timer2_counts() {
        // mov $fc,#2; mov $f1,#4; loop: mov a,$ff; beq loop
        let a = run(&[0x8F, 0x02, 0xFC, 0x8F, 0x04, 0xF1, 0xE4, 0xFF, 0xF0, 0xFC], 100);
        assert_eq!(a.a, 1);
    }
}
