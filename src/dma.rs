use crate::bus::Bus;

/// Шаблон смещений B-шины для режимов передачи 0..7.
const PATTERNS: [&[u8]; 8] = [
    &[0], &[0, 1], &[0, 0], &[0, 0, 1, 1], &[0, 1, 2, 3], &[0, 1, 0, 1], &[0, 0], &[0, 0, 1, 1],
];

#[derive(Clone, Copy)]
pub struct Channel {
    pub dmap: u8,
    pub bbad: u8,
    pub a1t: u16,
    pub a1b: u8,
    pub das: u16,
    pub dasb: u8,
    pub a2a: u16,
    pub ntrl: u8,
    pub unused: u8,
    pub hdma_on: bool,
    pub do_transfer: bool,
}

impl Default for Channel {
    fn default() -> Self {
        Channel {
            dmap: 0xFF, bbad: 0xFF, a1t: 0xFFFF, a1b: 0xFF, das: 0xFFFF, dasb: 0xFF,
            a2a: 0xFFFF, ntrl: 0xFF, unused: 0xFF, hdma_on: false, do_transfer: false,
        }
    }
}

impl Bus {
    pub fn dma_read(&self, off: u16) -> u8 {
        let c = &self.dma[((off >> 4) & 7) as usize];
        match off & 0xF {
            0 => c.dmap,
            1 => c.bbad,
            2 => c.a1t as u8,
            3 => (c.a1t >> 8) as u8,
            4 => c.a1b,
            5 => c.das as u8,
            6 => (c.das >> 8) as u8,
            7 => c.dasb,
            8 => c.a2a as u8,
            9 => (c.a2a >> 8) as u8,
            0xA => c.ntrl,
            _ => c.unused,
        }
    }

    pub fn dma_write(&mut self, off: u16, v: u8) {
        let c = &mut self.dma[((off >> 4) & 7) as usize];
        match off & 0xF {
            0 => c.dmap = v,
            1 => c.bbad = v,
            2 => c.a1t = (c.a1t & 0xFF00) | v as u16,
            3 => c.a1t = (c.a1t & 0x00FF) | (v as u16) << 8,
            4 => c.a1b = v,
            5 => c.das = (c.das & 0xFF00) | v as u16,
            6 => c.das = (c.das & 0x00FF) | (v as u16) << 8,
            7 => c.dasb = v,
            8 => c.a2a = (c.a2a & 0xFF00) | v as u16,
            9 => c.a2a = (c.a2a & 0x00FF) | (v as u16) << 8,
            0xA => c.ntrl = v,
            _ => c.unused = v,
        }
    }

    /// Обычный DMA ($420B). Выполняется мгновенно, возвращает число master-тактов,
    /// на которое нужно остановить CPU.
    pub fn run_gdma(&mut self, mask: u8) -> u32 {
        let mut cycles = 8;
        for i in 0..8 {
            if mask & (1 << i) == 0 { continue; }
            cycles += 8;
            let mut c = self.dma[i];
            let pat = PATTERNS[(c.dmap & 7) as usize];
            let mut left = if c.das == 0 { 0x10000u32 } else { c.das as u32 };
            let mut k = 0;
            while left > 0 {
                let a = ((c.a1b as u32) << 16) | c.a1t as u32;
                let b = c.bbad.wrapping_add(pat[k % pat.len()]);
                k += 1;
                if c.dmap & 0x80 != 0 {
                    let v = self.b_read(b);
                    self.write(a, v);
                } else {
                    let v = self.read(a);
                    self.b_write(b, v);
                }
                match (c.dmap >> 3) & 3 {
                    0 => c.a1t = c.a1t.wrapping_add(1),
                    2 => c.a1t = c.a1t.wrapping_sub(1),
                    _ => {}
                }
                left -= 1;
                cycles += 8;
            }
            c.das = 0;
            self.dma[i] = c;
        }
        cycles
    }

    /// Чтение очередной записи таблицы HDMA (счётчик строк и, для indirect, указатель).
    fn hdma_load(&mut self, c: &mut Channel) {
        let a = ((c.a1b as u32) << 16) | c.a2a as u32;
        c.ntrl = self.read(a);
        c.a2a = c.a2a.wrapping_add(1);
        if c.ntrl == 0 {
            c.hdma_on = false;
            return;
        }
        if c.dmap & 0x40 != 0 {
            let lo_a = ((c.a1b as u32) << 16) | c.a2a as u32;
            let hi_a = ((c.a1b as u32) << 16) | c.a2a.wrapping_add(1) as u32;
            let lo = self.read(lo_a) as u16;
            let hi = self.read(hi_a) as u16;
            c.das = lo | (hi << 8);
            c.a2a = c.a2a.wrapping_add(2);
        }
        c.do_transfer = true;
    }

    /// Начало кадра: инициализация активных каналов HDMA.
    pub fn hdma_init(&mut self) {
        for i in 0..8 {
            let mut c = self.dma[i];
            c.hdma_on = self.hdmaen & (1 << i) != 0;
            if c.hdma_on {
                c.a2a = c.a1t;
                self.hdma_load(&mut c);
                self.stall += 8;
            }
            self.dma[i] = c;
        }
    }

    /// Вызывается в начале каждой видимой строки.
    pub fn hdma_run(&mut self) {
        for i in 0..8 {
            let mut c = self.dma[i];
            if !c.hdma_on || self.hdmaen & (1 << i) == 0 { continue; }
            if c.do_transfer {
                let pat = PATTERNS[(c.dmap & 7) as usize];
                for k in 0..pat.len() {
                    let a = if c.dmap & 0x40 != 0 {
                        let a = ((c.dasb as u32) << 16) | c.das as u32;
                        c.das = c.das.wrapping_add(1);
                        a
                    } else {
                        let a = ((c.a1b as u32) << 16) | c.a2a as u32;
                        c.a2a = c.a2a.wrapping_add(1);
                        a
                    };
                    let b = c.bbad.wrapping_add(pat[k]);
                    if c.dmap & 0x80 != 0 {
                        let v = self.b_read(b);
                        self.write(a, v);
                    } else {
                        let v = self.read(a);
                        self.b_write(b, v);
                    }
                    self.stall += 8;
                }
                self.stall += 8;
            }
            // уменьшаем счётчик строк, бит 7 — режим повтора
            c.ntrl = (c.ntrl & 0x80) | (c.ntrl.wrapping_sub(1) & 0x7F);
            c.do_transfer = c.ntrl & 0x80 != 0;
            if c.ntrl & 0x7F == 0 {
                self.hdma_load(&mut c);
            }
            self.dma[i] = c;
        }
    }
}
