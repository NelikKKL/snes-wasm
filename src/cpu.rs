use crate::bus::Bus;

const C: u8 = 0x01;
const Z: u8 = 0x02;
const I: u8 = 0x04;
const D: u8 = 0x08;
const X: u8 = 0x10;
const M: u8 = 0x20;
const V: u8 = 0x40;
const N: u8 = 0x80;

#[derive(Clone, Copy, PartialEq)]
enum Am { Imm, Dp, DpX, DpY, Abs, AbsX, AbsY, Long, LongX, DpI, DpIX, DpIY, DpIL, DpILY, Sr, SrIY }

/// Процессор 65C816. `cycles` — master-такты: обращения к шине по их реальной
/// скорости (6/8/12) плюс грубая оценка внутренних тактов.
#[derive(Default)]
pub struct Cpu {
    pub a: u16, pub x: u16, pub y: u16, pub sp: u16, pub dp: u16, pub pc: u16,
    pub pbr: u8, pub dbr: u8, pub p: u8, pub emulation: bool,
    pub nmi: bool, pub irq: bool, pub waiting: bool, pub stopped: bool,
    cycles: u32,
    nw: bool, // «новые» стековые инструкции в режиме эмуляции не заворачивают S внутри страницы 1
}

impl Cpu {
    pub fn reset(&mut self, bus: &mut Bus) {
        *self = Cpu { emulation: true, p: 0x34, sp: 0x01FF, ..Default::default() };
        self.pc = self.rd16(bus, 0xFFFC);
    }

    pub fn debug(&self) -> String {
        format!(
            "{:02X}:{:04X} A={:04X} X={:04X} Y={:04X} SP={:04X} DP={:04X} DB={:02X} P={:02X} E={}",
            self.pbr, self.pc, self.a, self.x, self.y, self.sp, self.dp, self.dbr, self.p, self.emulation as u8
        )
    }

    // ---------- шина ----------
    fn rd8(&mut self, bus: &mut Bus, a: u32) -> u8 {
        let a = a & 0xFF_FFFF;
        self.cycles += bus.cost(a);
        bus.read(a)
    }
    fn wr8(&mut self, bus: &mut Bus, a: u32, v: u8) {
        let a = a & 0xFF_FFFF;
        self.cycles += bus.cost(a);
        bus.write(a, v)
    }
    fn rd16(&mut self, bus: &mut Bus, a: u32) -> u16 {
        let lo = self.rd8(bus, a) as u16;
        lo | (self.rd8(bus, a + 1) as u16) << 8
    }
    fn rd24(&mut self, bus: &mut Bus, a: u32) -> u32 {
        let lo = self.rd16(bus, a) as u32;
        lo | (self.rd8(bus, a + 2) as u32) << 16
    }
    fn rdv(&mut self, bus: &mut Bus, ea: u32, wide: bool) -> u16 {
        if wide { self.rd16(bus, ea) } else { self.rd8(bus, ea) as u16 }
    }
    fn wrv(&mut self, bus: &mut Bus, ea: u32, v: u16, wide: bool) {
        self.wr8(bus, ea, v as u8);
        if wide { self.wr8(bus, ea + 1, (v >> 8) as u8); }
    }
    fn fetch8(&mut self, bus: &mut Bus) -> u8 {
        let a = ((self.pbr as u32) << 16) | self.pc as u32;
        self.pc = self.pc.wrapping_add(1);
        self.rd8(bus, a)
    }
    fn fetch16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch8(bus) as u16;
        lo | (self.fetch8(bus) as u16) << 8
    }
    fn fetch24(&mut self, bus: &mut Bus) -> u32 {
        let lo = self.fetch16(bus) as u32;
        lo | (self.fetch8(bus) as u32) << 16
    }

    // ---------- стек ----------
    fn push8(&mut self, bus: &mut Bus, v: u8) {
        let a = self.sp as u32;
        self.wr8(bus, a, v);
        let sp = self.sp.wrapping_sub(1);
        self.sp = if self.emulation && !self.nw { 0x100 | (sp & 0xFF) } else { sp };
    }
    fn pull8(&mut self, bus: &mut Bus) -> u8 {
        let sp = self.sp.wrapping_add(1);
        self.sp = if self.emulation && !self.nw { 0x100 | (sp & 0xFF) } else { sp };
        let a = self.sp as u32;
        self.rd8(bus, a)
    }
    fn push16(&mut self, bus: &mut Bus, v: u16) { self.push8(bus, (v >> 8) as u8); self.push8(bus, v as u8); }
    fn pull16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.pull8(bus) as u16;
        lo | (self.pull8(bus) as u16) << 8
    }
    fn push_w(&mut self, bus: &mut Bus, v: u16, wide: bool) {
        if wide { self.push16(bus, v) } else { self.push8(bus, v as u8) }
    }
    fn pull_w(&mut self, bus: &mut Bus, wide: bool) -> u16 {
        if wide { self.pull16(bus) } else { self.pull8(bus) as u16 }
    }

    // ---------- флаги и регистры ----------
    fn m8(&self) -> bool { self.emulation || self.p & M != 0 }
    fn x8(&self) -> bool { self.emulation || self.p & X != 0 }
    fn set_flag(&mut self, f: u8, on: bool) { if on { self.p |= f } else { self.p &= !f } }
    fn set_p(&mut self, v: u8) {
        self.p = if self.emulation { v | 0x30 } else { v };
        if self.p & X != 0 { self.x &= 0xFF; self.y &= 0xFF; }
    }
    fn nz(&mut self, v: u16, wide: bool) {
        let (m, t) = if wide { (0xFFFF, 0x8000) } else { (0xFF, 0x80) };
        self.p &= !(Z | N);
        if v & m == 0 { self.p |= Z; }
        if v & t != 0 { self.p |= N; }
    }
    fn ac(&self, wide: bool) -> u16 { if wide { self.a } else { self.a & 0xFF } }
    fn set_a(&mut self, v: u16, wide: bool) {
        self.a = if wide { v } else { (self.a & 0xFF00) | (v & 0xFF) };
    }

    // ---------- адресация ----------
    /// dp+off+idx. В режиме эмуляции при DL=0 индексация заворачивается внутри страницы.
    fn dp_idx(&self, off: u16, idx: u16) -> u32 {
        if self.emulation && self.dp & 0xFF == 0 {
            (self.dp | (off.wrapping_add(idx) & 0xFF)) as u32
        } else {
            self.dp.wrapping_add(off).wrapping_add(idx) as u32
        }
    }

    fn ea(&mut self, bus: &mut Bus, am: Am, wide: bool) -> u32 {
        let dbr = (self.dbr as u32) << 16;
        match am {
            Am::Imm => {
                let a = ((self.pbr as u32) << 16) | self.pc as u32;
                self.pc = self.pc.wrapping_add(1 + wide as u16);
                a
            }
            Am::Dp => { let o = self.fetch8(bus) as u16; self.dp.wrapping_add(o) as u32 }
            Am::DpX => { let o = self.fetch8(bus) as u16; self.dp_idx(o, self.x) }
            Am::DpY => { let o = self.fetch8(bus) as u16; self.dp_idx(o, self.y) }
            Am::Abs => dbr | self.fetch16(bus) as u32,
            Am::AbsX => (dbr | self.fetch16(bus) as u32).wrapping_add(self.x as u32) & 0xFF_FFFF,
            Am::AbsY => (dbr | self.fetch16(bus) as u32).wrapping_add(self.y as u32) & 0xFF_FFFF,
            Am::Long => self.fetch24(bus),
            Am::LongX => self.fetch24(bus).wrapping_add(self.x as u32) & 0xFF_FFFF,
            Am::DpI => { let p = self.ea(bus, Am::Dp, false); dbr | self.rd16(bus, p) as u32 }
            Am::DpIX => {
                let p = self.ea(bus, Am::DpX, false);
                let lo = self.rd8(bus, p) as u32;
                // в режиме эмуляции старший байт указателя читается с переносом внутри страницы
                let hp = if self.emulation { (p & 0xFF00) | ((p + 1) & 0xFF) } else { (p + 1) & 0xFFFF };
                let hi = self.rd8(bus, hp) as u32;
                dbr | (hi << 8) | lo
            }
            Am::DpIY => { let p = self.ea(bus, Am::Dp, false); (dbr | self.rd16(bus, p) as u32).wrapping_add(self.y as u32) & 0xFF_FFFF }
            Am::DpIL => { let p = self.ea(bus, Am::Dp, false); self.rd24(bus, p) }
            Am::DpILY => { let p = self.ea(bus, Am::Dp, false); self.rd24(bus, p).wrapping_add(self.y as u32) & 0xFF_FFFF }
            Am::Sr => { let o = self.fetch8(bus) as u16; self.sp.wrapping_add(o) as u32 }
            Am::SrIY => {
                let o = self.fetch8(bus) as u16;
                let p = self.sp.wrapping_add(o) as u32;
                (dbr | self.rd16(bus, p) as u32).wrapping_add(self.y as u32) & 0xFF_FFFF
            }
        }
    }

    // ---------- арифметика ----------
    fn adc(&mut self, v: u16, wide: bool) {
        let (mask, sign) = if wide { (0xFFFFu32, 0x8000u32) } else { (0xFFu32, 0x80u32) };
        let a = self.ac(wide) as u32;
        let m = v as u32;
        let c = (self.p & C) as u32;
        let res;
        if self.p & D != 0 {
            let n: u32 = if wide { 4 } else { 2 };
            let (mut r, mut carry) = (0u32, c);
            for i in 0..n - 1 {
                let sh = i * 4;
                let mut d = ((a >> sh) & 0xF) + ((m >> sh) & 0xF) + carry;
                if d > 9 { d += 6; }
                carry = (d > 0xF) as u32;
                r |= (d & 0xF) << sh;
            }
            // старший разряд: V считается до десятичной поправки, по знаковой сумме
            let sh = (n - 1) * 4;
            let (at, mt) = ((a >> sh) & 0xF, (m >> sh) & 0xF);
            let sext = |x: u32| if x >= 8 { x as i32 - 16 } else { x as i32 };
            let top = sext(at) + sext(mt) + carry as i32;
            self.set_flag(V, top > 7 || top < -8);
            let mut d = at + mt + carry;
            if d > 9 { d += 6; }
            carry = (d > 0xF) as u32;
            r |= (d & 0xF) << sh;
            res = r;
            self.set_flag(C, carry != 0);
        } else {
            let bin = a + m + c;
            self.set_flag(V, (!(a ^ m) & (a ^ bin) & sign) != 0);
            res = bin & mask;
            self.set_flag(C, bin > mask);
        }
        self.set_a(res as u16, wide);
        self.nz(res as u16, wide);
    }

    fn sbc(&mut self, v: u16, wide: bool) {
        let mask: u16 = if wide { 0xFFFF } else { 0xFF };
        if self.p & D == 0 { return self.adc(!v & mask, wide); }
        let sign = if wide { 0x8000u32 } else { 0x80u32 };
        let a = self.ac(wide) as u32;
        let m = v as u32;
        let c = (self.p & C) as u32;
        let nm = (!v & mask) as u32;
        let bin = a + nm + c;
        self.set_flag(V, (!(a ^ nm) & (a ^ bin) & sign) != 0);
        let (mut r, mut borrow) = (0u32, 1 - c as i32);
        for i in 0..(if wide { 4 } else { 2 }) {
            let sh = i * 4;
            let mut d = ((a >> sh) & 0xF) as i32 - ((m >> sh) & 0xF) as i32 - borrow;
            if d < 0 { d += 10; borrow = 1 } else { borrow = 0 }
            r |= ((d as u32) & 0xF) << sh;
        }
        self.set_flag(C, borrow == 0);
        self.set_a(r as u16, wide);
        self.nz(r as u16, wide);
    }

    fn cmp(&mut self, r: u16, v: u16, wide: bool) {
        self.set_flag(C, r >= v);
        self.nz(r.wrapping_sub(v), wide);
    }

    fn alu(&mut self, aaa: u8, v: u16, wide: bool) {
        match aaa {
            0 => { let r = self.ac(wide) | v; self.set_a(r, wide); self.nz(r, wide); }
            1 => { let r = self.ac(wide) & v; self.set_a(r, wide); self.nz(r, wide); }
            2 => { let r = self.ac(wide) ^ v; self.set_a(r, wide); self.nz(r, wide); }
            3 => self.adc(v, wide),
            5 => { self.set_a(v, wide); self.nz(v, wide); }
            6 => { let a = self.ac(wide); self.cmp(a, v, wide); }
            _ => self.sbc(v, wide),
        }
    }

    /// kind: 0 ASL, 1 ROL, 2 LSR, 3 ROR, 6 DEC, 7 INC
    fn rmw_op(&mut self, kind: u8, v: u16, wide: bool) -> u16 {
        let (mask, top) = if wide { (0xFFFFu32, 0x8000u32) } else { (0xFFu32, 0x80u32) };
        let v = v as u32;
        let c = (self.p & C) as u32;
        let r = (match kind {
            0 => { self.set_flag(C, v & top != 0); v << 1 }
            1 => { self.set_flag(C, v & top != 0); (v << 1) | c }
            2 => { self.set_flag(C, v & 1 != 0); v >> 1 }
            3 => { self.set_flag(C, v & 1 != 0); (v >> 1) | if c != 0 { top } else { 0 } }
            6 => v.wrapping_sub(1),
            _ => v + 1,
        }) & mask;
        self.nz(r as u16, wide);
        r as u16
    }

    fn rmw_mem(&mut self, bus: &mut Bus, kind: u8, am: Am) {
        let w = !self.m8();
        let ea = self.ea(bus, am, w);
        let v = self.rdv(bus, ea, w);
        let r = self.rmw_op(kind, v, w);
        self.wrv(bus, ea, r, w);
    }

    fn rmw_acc(&mut self, kind: u8) {
        let w = !self.m8();
        let a = self.ac(w);
        let r = self.rmw_op(kind, a, w);
        self.set_a(r, w);
    }

    fn tsb(&mut self, bus: &mut Bus, am: Am, set: bool) {
        let w = !self.m8();
        let ea = self.ea(bus, am, w);
        let v = self.rdv(bus, ea, w);
        let a = self.ac(w);
        self.set_flag(Z, v & a == 0);
        let r = if set { v | a } else { v & !a };
        self.wrv(bus, ea, r, w);
    }

    fn bit(&mut self, bus: &mut Bus, am: Am) {
        let w = !self.m8();
        let ea = self.ea(bus, am, w);
        let v = self.rdv(bus, ea, w);
        let a = self.ac(w);
        self.set_flag(Z, v & a == 0);
        if am != Am::Imm {
            let (n, ov) = if w { (0x8000, 0x4000) } else { (0x80, 0x40) };
            self.set_flag(N, v & n != 0);
            self.set_flag(V, v & ov != 0);
        }
    }

    // r: 0 = STZ, 1 = STX, 2 = STY
    fn store(&mut self, bus: &mut Bus, am: Am, r: u8) {
        let w = if r == 0 { !self.m8() } else { !self.x8() };
        let ea = self.ea(bus, am, w);
        let v = match r { 0 => 0, 1 => self.x, _ => self.y };
        self.wrv(bus, ea, v, w);
    }

    fn load_idx(&mut self, bus: &mut Bus, am: Am, is_x: bool) {
        let w = !self.x8();
        let ea = self.ea(bus, am, w);
        let v = self.rdv(bus, ea, w);
        if is_x { self.x = v } else { self.y = v }
        self.nz(v, w);
    }

    fn cmp_idx(&mut self, bus: &mut Bus, am: Am, is_x: bool) {
        let w = !self.x8();
        let ea = self.ea(bus, am, w);
        let v = self.rdv(bus, ea, w);
        let r = if is_x { self.x } else { self.y };
        self.cmp(r, v, w);
    }

    fn inc_idx(&mut self, is_x: bool, d: u16) {
        let w = !self.x8();
        let r = (if is_x { self.x } else { self.y }).wrapping_add(d);
        let r = if w { r } else { r & 0xFF };
        if is_x { self.x = r } else { self.y = r }
        self.nz(r, w);
    }

    // ---------- переходы и прерывания ----------
    fn branch(&mut self, bus: &mut Bus, cond: bool) {
        let off = self.fetch8(bus) as i8;
        if cond { self.pc = self.pc.wrapping_add(off as i16 as u16); self.cycles += 6; }
    }

    fn br(&mut self, bus: &mut Bus, flag: u8, set: bool) {
        let cond = (self.p & flag != 0) == set;
        self.branch(bus, cond);
    }

    fn interrupt(&mut self, bus: &mut Bus, native_vec: u32, emu_vec: u32, brk: bool) {
        if !self.emulation { let b = self.pbr; self.push8(bus, b); }
        let pc = self.pc;
        self.push16(bus, pc);
        let mut p = self.p;
        if self.emulation { p = (p & !0x10) | 0x20 | if brk { 0x10 } else { 0 }; }
        self.push8(bus, p);
        self.p = (self.p | I) & !D;
        self.pbr = 0;
        let vec = if self.emulation { emu_vec } else { native_vec };
        self.pc = self.rd16(bus, vec);
    }

    // ---------- выполнение ----------
    /// Выполняет одну инструкцию (или вход в прерывание), возвращает число тактов.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        self.cycles = 0;
        if self.stopped { return 6; }
        if self.nmi {
            self.nmi = false;
            self.waiting = false;
            self.interrupt(bus, 0xFFEA, 0xFFFA, false);
            return self.cycles.max(1);
        }
        if self.irq {
            self.waiting = false;
            if self.p & I == 0 {
                self.interrupt(bus, 0xFFEE, 0xFFFE, false);
                return self.cycles.max(1);
            }
        }
        if self.waiting { return 6; }
        let op = self.fetch8(bus);
        self.exec(bus, op);
        self.cycles += 6; // грубая оценка внутренних тактов (точная таблица — позже)
        self.cycles.max(1)
    }

    fn exec(&mut self, bus: &mut Bus, op: u8) {
        use Am::*;
        let aaa = op >> 5;
        let bbb = ((op >> 2) & 7) as usize;

        // ORA AND EOR ADC STA LDA CMP SBC во всех режимах
        let alu_am = match op & 3 {
            1 if op != 0x89 => Some([DpIX, Dp, Imm, Abs, DpIY, DpX, AbsY, AbsX][bbb]),
            3 if bbb != 2 && bbb != 6 => Some([Sr, DpIL, Imm, Long, SrIY, DpILY, Imm, LongX][bbb]),
            2 if op & 0x1F == 0x12 => Some(DpI),
            _ => None,
        };
        if let Some(am) = alu_am {
            let w = !self.m8();
            let ea = self.ea(bus, am, w);
            if aaa == 4 {
                let a = self.a;
                self.wrv(bus, ea, a, w);
            } else {
                let v = self.rdv(bus, ea, w);
                self.alu(aaa, v, w);
            }
            return;
        }

        // PEA PEI PER PHD PLD PLB JSL RTL JSR(abs,X): S без завёртывания в странице 1
        self.nw = matches!(op, 0xF4 | 0xD4 | 0x62 | 0x0B | 0x2B | 0xAB | 0x22 | 0x6B | 0xFC);
        match op {
            // сдвиги, INC, DEC в памяти
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E | 0x46 | 0x4E | 0x56 | 0x5E
            | 0x66 | 0x6E | 0x76 | 0x7E | 0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let am = [Dp, Abs, DpX, AbsX][(bbb - 1) / 2];
                self.rmw_mem(bus, aaa, am);
            }
            0x0A | 0x2A | 0x4A | 0x6A => self.rmw_acc(aaa),
            0x1A => self.rmw_acc(7),
            0x3A => self.rmw_acc(6),

            0x04 => self.tsb(bus, Dp, true),
            0x0C => self.tsb(bus, Abs, true),
            0x14 => self.tsb(bus, Dp, false),
            0x1C => self.tsb(bus, Abs, false),
            0x24 => self.bit(bus, Dp),
            0x2C => self.bit(bus, Abs),
            0x34 => self.bit(bus, DpX),
            0x3C => self.bit(bus, AbsX),
            0x89 => self.bit(bus, Imm),

            0x64 => self.store(bus, Dp, 0),
            0x74 => self.store(bus, DpX, 0),
            0x9C => self.store(bus, Abs, 0),
            0x9E => self.store(bus, AbsX, 0),
            0x86 => self.store(bus, Dp, 1),
            0x8E => self.store(bus, Abs, 1),
            0x96 => self.store(bus, DpY, 1),
            0x84 => self.store(bus, Dp, 2),
            0x8C => self.store(bus, Abs, 2),
            0x94 => self.store(bus, DpX, 2),

            0xA2 => self.load_idx(bus, Imm, true),
            0xA6 => self.load_idx(bus, Dp, true),
            0xAE => self.load_idx(bus, Abs, true),
            0xB6 => self.load_idx(bus, DpY, true),
            0xBE => self.load_idx(bus, AbsY, true),
            0xA0 => self.load_idx(bus, Imm, false),
            0xA4 => self.load_idx(bus, Dp, false),
            0xAC => self.load_idx(bus, Abs, false),
            0xB4 => self.load_idx(bus, DpX, false),
            0xBC => self.load_idx(bus, AbsX, false),

            0xE0 => self.cmp_idx(bus, Imm, true),
            0xE4 => self.cmp_idx(bus, Dp, true),
            0xEC => self.cmp_idx(bus, Abs, true),
            0xC0 => self.cmp_idx(bus, Imm, false),
            0xC4 => self.cmp_idx(bus, Dp, false),
            0xCC => self.cmp_idx(bus, Abs, false),

            0xE8 => self.inc_idx(true, 1),
            0xC8 => self.inc_idx(false, 1),
            0xCA => self.inc_idx(true, 0xFFFF),
            0x88 => self.inc_idx(false, 0xFFFF),

            // ветвления
            0x10 => self.br(bus, N, false),
            0x30 => self.br(bus, N, true),
            0x50 => self.br(bus, V, false),
            0x70 => self.br(bus, V, true),
            0x90 => self.br(bus, C, false),
            0xB0 => self.br(bus, C, true),
            0xD0 => self.br(bus, Z, false),
            0xF0 => self.br(bus, Z, true),
            0x80 => self.branch(bus, true),
            0x82 => { let o = self.fetch16(bus); self.pc = self.pc.wrapping_add(o); }

            // переходы
            0x4C => self.pc = self.fetch16(bus),
            0x5C => { let a = self.fetch24(bus); self.pc = a as u16; self.pbr = (a >> 16) as u8; }
            0x6C => { let p = self.fetch16(bus) as u32; self.pc = self.rd16(bus, p); }
            0x7C => {
                let p = self.fetch16(bus).wrapping_add(self.x) as u32;
                let a = ((self.pbr as u32) << 16) | p;
                self.pc = self.rd16(bus, a);
            }
            0xDC => {
                let p = self.fetch16(bus) as u32;
                let a = self.rd24(bus, p);
                self.pc = a as u16;
                self.pbr = (a >> 16) as u8;
            }
            0x20 => { let a = self.fetch16(bus); let r = self.pc.wrapping_sub(1); self.push16(bus, r); self.pc = a; }
            0x22 => {
                let a = self.fetch24(bus);
                let b = self.pbr;
                self.push8(bus, b);
                let r = self.pc.wrapping_sub(1);
                self.push16(bus, r);
                self.pc = a as u16;
                self.pbr = (a >> 16) as u8;
            }
            0xFC => {
                let p = self.fetch16(bus).wrapping_add(self.x) as u32;
                let r = self.pc.wrapping_sub(1);
                self.push16(bus, r);
                let a = ((self.pbr as u32) << 16) | p;
                self.pc = self.rd16(bus, a);
            }
            0x60 => { self.pc = self.pull16(bus).wrapping_add(1); }
            0x6B => { self.pc = self.pull16(bus).wrapping_add(1); self.pbr = self.pull8(bus); }
            0x40 => {
                let p = self.pull8(bus);
                self.set_p(p);
                self.pc = self.pull16(bus);
                if !self.emulation { self.pbr = self.pull8(bus); }
            }
            0x00 => { self.fetch8(bus); self.interrupt(bus, 0xFFE6, 0xFFFE, true); }
            0x02 => { self.fetch8(bus); self.interrupt(bus, 0xFFE4, 0xFFF4, true); }

            // флаги
            0x18 => self.p &= !C,
            0x38 => self.p |= C,
            0x58 => self.p &= !I,
            0x78 => self.p |= I,
            0xB8 => self.p &= !V,
            0xD8 => self.p &= !D,
            0xF8 => self.p |= D,
            0xC2 => { let m = self.fetch8(bus); let p = self.p & !m; self.set_p(p); }
            0xE2 => { let m = self.fetch8(bus); let p = self.p | m; self.set_p(p); }
            0xFB => {
                let c = self.p & C != 0;
                let e = self.emulation;
                self.emulation = c;
                self.set_flag(C, e);
                if self.emulation {
                    self.p |= 0x30;
                    self.x &= 0xFF;
                    self.y &= 0xFF;
                    self.sp = 0x100 | (self.sp & 0xFF);
                }
            }

            // пересылки
            0xAA | 0xA8 => {
                let w = !self.x8();
                let v = if w { self.a } else { self.a & 0xFF };
                if op == 0xAA { self.x = v } else { self.y = v }
                self.nz(v, w);
            }
            0x8A | 0x98 => {
                let w = !self.m8();
                let v = if op == 0x8A { self.x } else { self.y };
                self.set_a(v, w);
                self.nz(v, w);
            }
            0xBA => {
                let w = !self.x8();
                let v = if w { self.sp } else { self.sp & 0xFF };
                self.x = v;
                self.nz(v, w);
            }
            0x9A => self.sp = if self.emulation { 0x100 | (self.x & 0xFF) } else { self.x },
            0x9B => { let w = !self.x8(); self.y = self.x; let v = self.y; self.nz(v, w); }
            0xBB => { let w = !self.x8(); self.x = self.y; let v = self.x; self.nz(v, w); }
            0x5B => { self.dp = self.a; let v = self.dp; self.nz(v, true); }
            0x7B => { self.a = self.dp; let v = self.a; self.nz(v, true); }
            0x1B => self.sp = if self.emulation { 0x100 | (self.a & 0xFF) } else { self.a },
            0x3B => { self.a = self.sp; let v = self.a; self.nz(v, true); }
            0xEB => { self.a = self.a.swap_bytes(); let v = self.a; self.nz(v, false); }

            // стек
            0x48 => { let (v, w) = (self.a, !self.m8()); self.push_w(bus, v, w); }
            0x68 => { let w = !self.m8(); let v = self.pull_w(bus, w); self.set_a(v, w); self.nz(v, w); }
            0xDA => { let (v, w) = (self.x, !self.x8()); self.push_w(bus, v, w); }
            0x5A => { let (v, w) = (self.y, !self.x8()); self.push_w(bus, v, w); }
            0xFA => { let w = !self.x8(); self.x = self.pull_w(bus, w); let v = self.x; self.nz(v, w); }
            0x7A => { let w = !self.x8(); self.y = self.pull_w(bus, w); let v = self.y; self.nz(v, w); }
            0x8B => { let v = self.dbr; self.push8(bus, v); }
            0xAB => { self.dbr = self.pull8(bus); let v = self.dbr as u16; self.nz(v, false); }
            0x0B => { let v = self.dp; self.push16(bus, v); }
            0x2B => { self.dp = self.pull16(bus); let v = self.dp; self.nz(v, true); }
            0x4B => { let v = self.pbr; self.push8(bus, v); }
            0x08 => { let v = self.p; self.push8(bus, v); }
            0x28 => { let v = self.pull8(bus); self.set_p(v); }
            0xF4 => { let v = self.fetch16(bus); self.push16(bus, v); }
            0xD4 => { let a = self.ea(bus, Dp, false); let v = self.rd16(bus, a); self.push16(bus, v); }
            0x62 => { let o = self.fetch16(bus); let v = self.pc.wrapping_add(o); self.push16(bus, v); }

            // блочные пересылки: MVP (0x44) / MVN (0x54)
            0x44 | 0x54 => {
                let dst = self.fetch8(bus);
                let src = self.fetch8(bus);
                self.dbr = dst;
                let (x, y) = (self.x, self.y);
                let v = self.rd8(bus, ((src as u32) << 16) | x as u32);
                self.wr8(bus, ((dst as u32) << 16) | y as u32, v);
                let d: u16 = if op == 0x54 { 1 } else { 0xFFFF };
                let m: u16 = if self.x8() { 0xFF } else { 0xFFFF };
                self.x = x.wrapping_add(d) & m;
                self.y = y.wrapping_add(d) & m;
                self.a = self.a.wrapping_sub(1);
                if self.a != 0xFFFF { self.pc = self.pc.wrapping_sub(3); }
            }

            0xCB => self.waiting = true,
            0xDB => self.stopped = true,
            0x42 => { self.fetch8(bus); }
            _ => {} // 0xEA NOP
        }
        if self.nw {
            self.nw = false;
            if self.emulation { self.sp = 0x100 | (self.sp & 0xFF); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    /// Кладём программу в WRAM (7E:0000) и выполняем `steps` инструкций.
    fn run(prog: &[u8], steps: usize) -> Cpu {
        let cart = Cartridge::from_bytes(&vec![0u8; 0x8000]).unwrap();
        let mut bus = Bus::new(cart);
        bus.wram[..prog.len()].copy_from_slice(prog);
        let mut cpu = Cpu::default();
        cpu.reset(&mut bus);
        cpu.pbr = 0x7E;
        cpu.pc = 0;
        for _ in 0..steps { cpu.step(&mut bus); }
        cpu
    }

    #[test]
    fn adc_8bit_flags() {
        // CLC; LDA #$50; ADC #$50
        let c = run(&[0x18, 0xA9, 0x50, 0x69, 0x50], 3);
        assert_eq!(c.a & 0xFF, 0xA0);
        assert!(c.p & V != 0 && c.p & N != 0 && c.p & C == 0);
    }

    #[test]
    fn native_16bit_adc() {
        // CLC; XCE; REP #$30; CLC; LDA #$1234; ADC #$1111
        let c = run(&[0x18, 0xFB, 0xC2, 0x30, 0x18, 0xA9, 0x34, 0x12, 0x69, 0x11, 0x11], 6);
        assert!(!c.emulation);
        assert_eq!(c.a, 0x2345);
    }

    #[test]
    fn dex_bne_loop() {
        // LDX #3; loop: DEX; BNE loop
        let c = run(&[0xA2, 0x03, 0xCA, 0xD0, 0xFD], 7);
        assert_eq!(c.x, 0);
        assert_eq!(c.pc, 5);
    }

    #[test]
    fn jsr_rts() {
        let prog = [0x20, 0x07, 0x00, 0xA2, 0x01, 0xEA, 0xEA, 0xA9, 0x42, 0x60];
        let c = run(&prog, 4);
        assert_eq!(c.a & 0xFF, 0x42);
        assert_eq!(c.x, 1);
        assert_eq!(c.pc, 5);
        assert_eq!(c.sp, 0x01FF);
    }

    #[test]
    fn bcd_overflow_flag() {
        // SED; SEC; LDA #$79; ADC #$00 → $80, V=1 (классический случай десятичного режима)
        let c = run(&[0xF8, 0x38, 0xA9, 0x79, 0x69, 0x00], 4);
        assert_eq!(c.a & 0xFF, 0x80);
        assert!(c.p & V != 0 && c.p & N != 0);
    }

    #[test]
    fn emulation_dp_indexed_wraps_in_page() {
        // LDA #$42; STA $08; LDX #$10; LDA $F8,X → адрес ($F8+$10)&$FF = $08, а не $108
        let c = run(&[0xA9, 0x42, 0x85, 0x08, 0xA2, 0x10, 0xB5, 0xF8], 4);
        assert_eq!(c.a & 0xFF, 0x42);
    }

    #[test]
    fn bcd_adc() {
        // SED; CLC; LDA #$19; ADC #$28
        let c = run(&[0xF8, 0x18, 0xA9, 0x19, 0x69, 0x28], 4);
        assert_eq!(c.a & 0xFF, 0x47);
    }
}
