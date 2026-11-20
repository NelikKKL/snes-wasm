//! NEC µPD7725 — процессор внутри DSP-1/2/3/4 на картриджах.
//! Прошивку (8192 байта: 2048 24-битных слов программы + 1024 16-битных слова данных)
//! эмулятор не содержит — её нужно загрузить отдельно (`load_firmware`).

const RQM: u16 = 0x8000; // запрос обмена для хозяина (SNES)
const DRS: u16 = 0x1000; // какой байт регистра данных следующий
const DRC: u16 = 0x0400; // 1 = обмен 8-битный, 0 = 16-битный

#[derive(Clone, Copy, Default)]
struct Flags {
    ov0: bool,
    ov1: bool,
    z: bool,
    c: bool,
    s0: bool,
    s1: bool,
}

pub struct Upd7725 {
    program: Vec<u32>,
    data: Vec<u16>,
    ram: [u16; 256],
    pc: u16,
    rp: u16,
    dp: u16,
    stack: [u16; 4],
    sp: usize,
    a: u16,
    b: u16,
    k: i16,
    l: i16,
    m: u16,
    n: u16,
    tr: u16,
    trb: u16,
    dr: u16,
    sr: u16,
    flag_a: Flags,
    flag_b: Flags,
    acc: u64,
    loaded: bool,
}

/// Допустимые коды ветвления (BRCH) команды JP — по ним определяем порядок байт в прошивке.
fn jp_valid(brch: u32) -> bool {
    match brch {
        0x000 | 0x0B0 | 0x0B1 | 0x0B2 | 0x0B4 | 0x0B8 | 0x0BA | 0x0BC | 0x0BE | 0x0C0 | 0x0C2 | 0x100 | 0x140 => true,
        0x080..=0x0AE => brch & 1 == 0,
        _ => false,
    }
}

impl Upd7725 {
    pub fn new() -> Self {
        Upd7725 {
            program: vec![0; 2048],
            data: vec![0; 1024],
            ram: [0; 256],
            pc: 0, rp: 0, dp: 0, stack: [0; 4], sp: 0,
            a: 0, b: 0, k: 0, l: 0, m: 0, n: 0, tr: 0, trb: 0, dr: 0, sr: 0,
            flag_a: Flags::default(), flag_b: Flags::default(),
            acc: 0, loaded: false,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Загружает прошивку; порядок байт (LE/BE) в разных дампах разный — определяется сам.
    pub fn load_firmware(&mut self, data: &[u8]) -> Result<(), String> {
        if data.len() != 8192 {
            return Err(format!("ожидался файл прошивки на 8192 байта, а не {}", data.len()));
        }
        let decode = |le: bool| -> (Vec<u32>, Vec<u16>) {
            let prog = (0..2048)
                .map(|i| {
                    let b = &data[i * 3..i * 3 + 3];
                    if le {
                        b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16
                    } else {
                        (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32
                    }
                })
                .collect();
            let dat = (0..1024)
                .map(|i| {
                    let b = &data[6144 + i * 2..6144 + i * 2 + 2];
                    if le { b[0] as u16 | (b[1] as u16) << 8 } else { (b[0] as u16) << 8 | b[1] as u16 }
                })
                .collect();
            (prog, dat)
        };
        let score = |p: &[u32]| -> usize {
            let (mut total, mut valid) = (0usize, 0usize);
            for &w in p {
                if w >> 22 == 2 {
                    total += 1;
                    if jp_valid((w >> 13) & 0x1FF) {
                        valid += 1;
                    }
                }
            }
            if total >= 4 && valid * 10 >= total * 9 { valid } else { 0 }
        };
        let (pl, dl) = decode(true);
        let (pb, db) = decode(false);
        let (sl, sb) = (score(&pl), score(&pb));
        if sl == 0 && sb == 0 {
            return Err("это не похоже на прошивку DSP-1/2/3/4 (µPD7725)".into());
        }
        let (p, d) = if sl >= sb { (pl, dl) } else { (pb, db) };
        *self = Upd7725::new();
        self.program = p;
        self.data = d;
        self.loaded = true;
        Ok(())
    }

    #[cfg(test)]
    pub fn with_program(words: &[u32]) -> Self {
        let mut d = Upd7725::new();
        d.program[..words.len()].copy_from_slice(words);
        d.loaded = true;
        d
    }

    // ---------- обмен с S-CPU ----------
    pub fn read_sr(&self) -> u8 {
        (self.sr >> 8) as u8
    }

    pub fn read_dr(&mut self) -> u8 {
        if self.sr & DRC == 0 {
            if self.sr & DRS == 0 {
                self.sr |= DRS;
                self.dr as u8
            } else {
                self.sr &= !(RQM | DRS);
                (self.dr >> 8) as u8
            }
        } else {
            self.sr &= !RQM;
            self.dr as u8
        }
    }

    pub fn write_dr(&mut self, v: u8) {
        if self.sr & DRC == 0 {
            if self.sr & DRS == 0 {
                self.sr |= DRS;
                self.dr = (self.dr & 0xFF00) | v as u16;
            } else {
                self.sr &= !(RQM | DRS);
                self.dr = ((v as u16) << 8) | (self.dr & 0x00FF);
            }
        } else {
            self.sr &= !RQM;
            self.dr = (self.dr & 0xFF00) | v as u16;
        }
    }

    /// Прокрутить на `master` master-тактов S-CPU (чип работает на 7.6 МГц, 1 инструкция/такт).
    pub fn clock(&mut self, master: u32) {
        if !self.loaded {
            return;
        }
        self.acc += master as u64 * 7_600_000;
        let n = self.acc / 21_477_272;
        self.acc %= 21_477_272;
        for _ in 0..n {
            self.step();
        }
    }

    // ---------- ядро ----------
    pub fn step(&mut self) {
        if !self.loaded {
            return;
        }
        let op = self.program[(self.pc & 0x7FF) as usize];
        self.pc = (self.pc + 1) & 0x7FF;
        match op >> 22 {
            0 => self.exec_op(op),
            1 => {
                // RT: операция + возврат
                self.exec_op(op);
                self.sp = self.sp.wrapping_sub(1) & 3;
                self.pc = self.stack[self.sp];
            }
            2 => self.exec_jp(op),
            _ => {
                let id = (op >> 6) as u16;
                self.write_dst((op & 0xF) as u8, id);
            }
        }
        // умножитель: M:N = K*L*2 пересчитывается после каждой инструкции
        let r = self.k as i32 * self.l as i32;
        self.m = (r >> 15) as u16;
        self.n = (r << 1) as u16;
    }

    fn write_dst(&mut self, dst: u8, v: u16) {
        match dst {
            0 => {}
            1 => self.a = v,
            2 => self.b = v,
            3 => self.tr = v,
            4 => self.dp = v & 0xFF,
            5 => self.rp = v & 0x3FF,
            6 => {
                self.dr = v;
                self.sr |= RQM;
            }
            7 => self.sr = (self.sr & 0x907C) | (v & !0x907C),
            8 | 9 => {} // последовательный вывод не используется
            10 => self.k = v as i16,
            11 => {
                self.k = v as i16;
                self.l = self.data[(self.rp & 0x3FF) as usize] as i16;
            }
            12 => {
                self.l = v as i16;
                self.k = self.ram[((self.dp | 0x40) & 0xFF) as usize] as i16;
            }
            13 => self.l = v as i16,
            14 => self.trb = v,
            _ => self.ram[(self.dp & 0xFF) as usize] = v,
        }
    }

    fn exec_op(&mut self, op: u32) {
        let pselect = (op >> 20) & 3;
        let alu = (op >> 16) & 15;
        let asl = (op >> 15) & 1;
        let dpl = (op >> 13) & 3;
        let dphm = ((op >> 9) & 15) as u16;
        let rpdcr = (op >> 8) & 1;
        let src = ((op >> 4) & 15) as u8;
        let dst = (op & 15) as u8;

        let idb: u16 = match src {
            0 => self.trb,
            1 => self.a,
            2 => self.b,
            3 => self.tr,
            4 => self.dp,
            5 => self.rp,
            6 => self.data[(self.rp & 0x3FF) as usize],
            7 => 0x8000u16.wrapping_sub(self.flag_a.s1 as u16),
            8 => {
                self.sr |= RQM;
                self.dr
            }
            9 => self.dr,
            10 => self.sr,
            11 | 12 => 0, // последовательный ввод не используется
            13 => self.k as u16,
            14 => self.l as u16,
            _ => self.ram[(self.dp & 0xFF) as usize],
        };

        if alu != 0 {
            let mut p: u16 = match pselect {
                0 => self.ram[(self.dp & 0xFF) as usize],
                1 => idb,
                2 => self.m,
                _ => self.n,
            };
            let (q, mut flag, c) = if asl == 0 {
                (self.a, self.flag_a, self.flag_b.c)
            } else {
                (self.b, self.flag_b, self.flag_a.c)
            };
            let r: u16 = match alu {
                1 => q | p,
                2 => q & p,
                3 => q ^ p,
                4 => q.wrapping_sub(p),
                5 => q.wrapping_add(p),
                6 => q.wrapping_sub(p).wrapping_sub(c as u16),
                7 => q.wrapping_add(p).wrapping_add(c as u16),
                8 => { p = 1; q.wrapping_sub(1) }
                9 => { p = 1; q.wrapping_add(1) }
                10 => !q,
                11 => (q >> 1) | (q & 0x8000),
                12 => (q << 1) | c as u16,
                13 => (q << 2) | 3,
                14 => (q << 4) | 15,
                _ => (q << 8) | (q >> 8),
            };
            flag.s0 = r & 0x8000 != 0;
            flag.z = r == 0;
            match alu {
                1 | 2 | 3 | 10 | 13 | 14 | 15 => {
                    flag.c = false;
                    flag.ov0 = false;
                    flag.ov1 = false;
                }
                4..=9 => {
                    if alu & 1 == 1 {
                        flag.ov0 = (q ^ r) & !(q ^ p) & 0x8000 != 0;
                        flag.c = r < q;
                    } else {
                        flag.ov0 = (q ^ r) & (q ^ p) & 0x8000 != 0;
                        flag.c = r > q;
                    }
                    if flag.ov0 {
                        // S1 — знак «настоящего» результата для насыщения (SGN = 0x8000 − S1)
                        flag.s1 = flag.ov1 != (r & 0x8000 != 0);
                        flag.ov1 = !flag.ov1;
                    }
                }
                11 => {
                    flag.c = q & 1 != 0;
                    flag.ov0 = false;
                    flag.ov1 = false;
                }
                _ => {
                    flag.c = q >> 15 != 0;
                    flag.ov0 = false;
                    flag.ov1 = false;
                }
            }
            if asl == 0 {
                self.a = r;
                self.flag_a = flag;
            } else {
                self.b = r;
                self.flag_b = flag;
            }
        }

        self.write_dst(dst, idb);

        match dpl {
            1 => self.dp = (self.dp & 0xF0) | ((self.dp + 1) & 0x0F),
            2 => self.dp = (self.dp & 0xF0) | (self.dp.wrapping_sub(1) & 0x0F),
            3 => self.dp &= 0xF0,
            _ => {}
        }
        self.dp = (self.dp ^ (dphm << 4)) & 0xFF;
        if rpdcr == 1 {
            self.rp = self.rp.wrapping_sub(1) & 0x3FF;
        }
    }

    fn exec_jp(&mut self, op: u32) {
        let brch = (op >> 13) & 0x1FF;
        let na = ((op >> 2) & 0x7FF) as u16;
        let (a, b) = (self.flag_a, self.flag_b);
        let dpl = self.dp & 0x0F;
        let jump = match brch {
            0x000 => true, // JMPSO: адрес из последовательного вывода (не используется → 0)
            0x080 => !a.c,
            0x082 => a.c,
            0x084 => !b.c,
            0x086 => b.c,
            0x088 => !a.z,
            0x08A => a.z,
            0x08C => !b.z,
            0x08E => b.z,
            0x090 => !a.ov0,
            0x092 => a.ov0,
            0x094 => !b.ov0,
            0x096 => b.ov0,
            0x098 => !a.ov1,
            0x09A => a.ov1,
            0x09C => !b.ov1,
            0x09E => b.ov1,
            0x0A0 => !a.s0,
            0x0A2 => a.s0,
            0x0A4 => !b.s0,
            0x0A6 => b.s0,
            0x0A8 => !a.s1,
            0x0AA => a.s1,
            0x0AC => !b.s1,
            0x0AE => b.s1,
            0x0B0 => dpl == 0,
            0x0B1 => dpl != 0,
            0x0B2 => dpl == 0x0F,
            0x0B4 => dpl != 0x0F,
            0x0B8 => true,  // JNSIAK: серийный интерфейс не используется
            0x0BA => false, // JSIAK
            0x0BC => true,  // JNSOAK
            0x0BE => false, // JSOAK
            0x0C0 => self.sr & RQM == 0,
            0x0C2 => self.sr & RQM != 0,
            0x100 => true, // LJMP
            0x140 => {
                // LCALL
                self.stack[self.sp] = self.pc;
                self.sp = (self.sp + 1) & 3;
                true
            }
            _ => false,
        };
        if jump {
            self.pc = if brch == 0x000 { 0 } else { na };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ld(imm: u16, dst: u32) -> u32 {
        0xC0_0000 | ((imm as u32) << 6) | dst
    }
    /// OP: pselect, alu, asl, src, dst (DPL/DPH/RPDCR не используются)
    fn op(pselect: u32, alu: u32, asl: u32, src: u32, dst: u32) -> u32 {
        (pselect << 20) | (alu << 16) | (asl << 15) | (src << 4) | dst
    }
    fn jp(brch: u32, na: u32) -> u32 {
        0x80_0000 | (brch << 13) | (na << 2)
    }
    fn run(words: &[u32], steps: usize) -> Upd7725 {
        let mut d = Upd7725::with_program(words);
        for _ in 0..steps {
            d.step();
        }
        d
    }

    #[test]
    fn load_and_add() {
        // A = $1234; B = 1; A = A + B
        let d = run(&[ld(0x1234, 1), ld(1, 2), op(1, 5, 0, 2, 0)], 3);
        assert_eq!(d.a, 0x1235);
        assert!(!d.flag_a.z && !d.flag_a.s0);
    }

    #[test]
    fn overflow_sets_saturation_sign() {
        // $7FFF + 1 переполняется → SGN должен дать $7FFF (положительное насыщение)
        let d = run(&[ld(0x7FFF, 1), ld(1, 2), op(1, 5, 0, 2, 0), op(0, 0, 0, 7, 3)], 4);
        assert!(d.flag_a.ov0);
        assert_eq!(d.tr, 0x7FFF);
        // $8000 + $8000 → отрицательное насыщение, SGN = $8000
        let d = run(&[ld(0x8000, 1), ld(0x8000, 2), op(1, 5, 0, 2, 0), op(0, 0, 0, 7, 3)], 4);
        assert_eq!(d.tr, 0x8000);
    }

    #[test]
    fn multiplier_updates_every_instruction() {
        // K = L = $4000 → K*L*2 = $20000000: M = $2000, N = 0
        let d = run(&[ld(0x4000, 10), ld(0x4000, 13)], 2);
        assert_eq!((d.m, d.n), (0x2000, 0));
    }

    #[test]
    fn long_jump_skips_code() {
        let d = run(&[jp(0x100, 5), ld(0xFFFF, 1), ld(0xFFFF, 1), ld(0xFFFF, 1), ld(0xFFFF, 1), ld(7, 1)], 2);
        assert_eq!(d.a, 7);
    }

    #[test]
    fn firmware_loader_detects_byte_order() {
        // программа из шестнадцати LJMP; одни и те же слова в разном порядке байт
        let words: Vec<u32> = (0..16).map(|i| jp(0x100, i)).collect();
        for le in [true, false] {
            let mut img = vec![0u8; 8192];
            for (i, w) in words.iter().enumerate() {
                let b = if le { [*w as u8, (*w >> 8) as u8, (*w >> 16) as u8] } else { [(*w >> 16) as u8, (*w >> 8) as u8, *w as u8] };
                img[i * 3..i * 3 + 3].copy_from_slice(&b);
            }
            let mut d = Upd7725::new();
            d.load_firmware(&img).unwrap();
            assert_eq!(d.program[3], words[3], "le = {}", le);
        }
        assert!(Upd7725::new().load_firmware(&vec![0u8; 8192]).is_err());
        assert!(Upd7725::new().load_firmware(&[0u8; 100]).is_err());
    }
}
