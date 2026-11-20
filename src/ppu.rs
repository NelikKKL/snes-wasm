pub const W: usize = 256;
pub const H: usize = 224;

/// (слой, приоритет) от переднего плана к заднему. Слои: 0..3 = BG1..BG4, 4 = OBJ.
type Order = &'static [(u8, u8)];
const ORDER_M0: Order = &[(4, 3), (0, 1), (1, 1), (4, 2), (0, 0), (1, 0), (4, 1), (2, 1), (3, 1), (4, 0), (2, 0), (3, 0)];
const ORDER_M1: Order = &[(4, 3), (0, 1), (1, 1), (4, 2), (0, 0), (1, 0), (4, 1), (2, 1), (4, 0), (2, 0)];
const ORDER_M1_BG3: Order = &[(2, 1), (4, 3), (0, 1), (1, 1), (4, 2), (0, 0), (1, 0), (4, 1), (4, 0), (2, 0)];
const ORDER_M2: Order = &[(4, 3), (0, 1), (4, 2), (1, 1), (4, 1), (0, 0), (4, 0), (1, 0)];
const ORDER_M7: Order = &[(4, 3), (4, 2), (4, 1), (0, 0), (4, 0)];

/// Глубина цвета BG1..BG4 по режимам 0..6 (0 = слоя нет).
const BPP: [[u8; 4]; 7] = [[2, 2, 2, 2], [4, 4, 2, 0], [4, 4, 0, 0], [8, 4, 0, 0], [8, 2, 0, 0], [4, 2, 0, 0], [4, 0, 0, 0]];

/// Размеры спрайтов (малый, большой) для OBSEL bits 5-7.
const OBJ_SIZES: [[(usize, usize); 2]; 8] = [
    [(8, 8), (16, 16)], [(8, 8), (32, 32)], [(8, 8), (64, 64)], [(16, 16), (32, 32)],
    [(16, 16), (64, 64)], [(32, 32), (64, 64)], [(16, 32), (32, 64)], [(16, 32), (32, 32)],
];

pub struct Ppu {
    pub fb: Vec<u8>, // RGBA 256x224
    vram: Vec<u16>,  // 32K слов
    cgram: [u16; 256],
    oam: [u8; 544],
    inidisp: u8,
    obsel: u8,
    oam_addr: u16, // байтовый адрес, бит 9 = high-таблица
    oam_latch: u8,
    bgmode: u8,
    bgsc: [u8; 4],
    bgnba: [u8; 2],
    hofs: [u16; 4],
    vofs: [u16; 4],
    ofs_latch: u8,
    vmain: u8,
    vmadd: u16,
    vram_latch: u16,
    cgadd: u8,
    cg_latch: u8,
    cg_high: bool,
    tm: u8,
    ts: u8,
    tmw: u8,
    tsw: u8,
    w12sel: u8,
    w34sel: u8,
    wobjsel: u8,
    wh: [u8; 4],
    wbglog: u8,
    wobjlog: u8,
    cgwsel: u8,
    cgadsub: u8,
    fixed: u16, // фиксированный цвет COLDATA (BGR555)
    // Mode 7
    m7sel: u8,
    m7: [i16; 4], // A, B, C, D (8.8)
    m7x: i16,
    m7y: i16,
    m7hofs: i16,
    m7vofs: i16,
    m7_latch: u8,
    mosaic: u8,
    hlatch: u16,
    vlatch: u16,
    h_toggle: bool,
    v_toggle: bool,
    latched: bool,
}

fn sext13(w: u16) -> i16 {
    ((w << 3) as i16) >> 3
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            fb: vec![0; W * H * 4], vram: vec![0; 0x8000], cgram: [0; 256], oam: [0; 544],
            inidisp: 0x80, obsel: 0, oam_addr: 0, oam_latch: 0, bgmode: 0, bgsc: [0; 4], bgnba: [0; 2],
            hofs: [0; 4], vofs: [0; 4], ofs_latch: 0, vmain: 0, vmadd: 0, vram_latch: 0,
            cgadd: 0, cg_latch: 0, cg_high: false, tm: 0,
            ts: 0, tmw: 0, tsw: 0, w12sel: 0, w34sel: 0, wobjsel: 0, wh: [0; 4],
            wbglog: 0, wobjlog: 0, cgwsel: 0, cgadsub: 0, fixed: 0,
            m7sel: 0, m7: [0; 4], m7x: 0, m7y: 0, m7hofs: 0, m7vofs: 0, m7_latch: 0,
            mosaic: 0, hlatch: 0, vlatch: 0, h_toggle: false, v_toggle: false, latched: false,
        }
    }

    // ---------- регистры ----------
    /// Чтение $2137: фиксирует текущие H (точки) и V (строка).
    pub fn latch_counters(&mut self, h: u16, v: u16) {
        self.hlatch = h;
        self.vlatch = v;
        self.latched = true;
    }

    fn vinc(&self) -> u16 {
        match self.vmain & 3 { 0 => 1, 1 => 32, _ => 128 }
    }

    fn prefetch(&mut self) {
        self.vram_latch = self.vram[(self.vmadd & 0x7FFF) as usize];
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x00 => self.inidisp = v,
            0x01 => self.obsel = v,
            0x02 => self.oam_addr = (self.oam_addr & 0x200) | (v as u16) << 1,
            0x03 => self.oam_addr = (self.oam_addr & 0x1FF) | ((v & 1) as u16) << 9,
            0x04 => {
                let a = self.oam_addr as usize;
                if a & 0x200 != 0 {
                    self.oam[0x200 + (a & 0x1F)] = v;
                } else if a & 1 == 0 {
                    self.oam_latch = v;
                } else {
                    self.oam[a - 1] = self.oam_latch;
                    self.oam[a] = v;
                }
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
            }
            0x05 => self.bgmode = v,
            0x06 => self.mosaic = v, // биты 7-4: размер−1, биты 3-0: BG1..BG4
            0x07..=0x0A => self.bgsc[(reg - 7) as usize] = v,
            0x0B => self.bgnba[0] = v,
            0x0C => self.bgnba[1] = v,
            0x0D..=0x14 => {
                let i = ((reg - 0x0D) / 2) as usize;
                if reg & 1 == 1 {
                    // не обрезаем до 10 бит: вторая запись берёт 3 бита из старшего байта первой
                    self.hofs[i] = ((v as u16) << 8) | (self.ofs_latch & 0xF8) as u16 | ((self.hofs[i] >> 8) & 7);
                } else {
                    self.vofs[i] = ((v as u16) << 8) | self.ofs_latch as u16;
                }
                self.ofs_latch = v;
                // M7HOFS/M7VOFS делят адреса с BG1HOFS/BG1VOFS, но имеют свою защёлку
                if reg == 0x0D || reg == 0x0E {
                    let w = sext13(((v as u16) << 8) | self.m7_latch as u16);
                    if reg == 0x0D { self.m7hofs = w } else { self.m7vofs = w }
                    self.m7_latch = v;
                }
            }
            0x15 => self.vmain = v,
            0x16 => { self.vmadd = (self.vmadd & 0xFF00) | v as u16; self.prefetch(); }
            0x17 => { self.vmadd = (self.vmadd & 0x00FF) | (v as u16) << 8; self.prefetch(); }
            0x18 => {
                let a = (self.vmadd & 0x7FFF) as usize;
                self.vram[a] = (self.vram[a] & 0xFF00) | v as u16;
                if self.vmain & 0x80 == 0 { self.vmadd = self.vmadd.wrapping_add(self.vinc()); }
            }
            0x19 => {
                let a = (self.vmadd & 0x7FFF) as usize;
                self.vram[a] = (self.vram[a] & 0x00FF) | (v as u16) << 8;
                if self.vmain & 0x80 != 0 { self.vmadd = self.vmadd.wrapping_add(self.vinc()); }
            }
            0x1A => self.m7sel = v,
            0x1B..=0x1E => {
                self.m7[(reg - 0x1B) as usize] = (((v as u16) << 8) | self.m7_latch as u16) as i16;
                self.m7_latch = v;
            }
            0x1F | 0x20 => {
                let w = sext13(((v as u16) << 8) | self.m7_latch as u16);
                if reg == 0x1F { self.m7x = w } else { self.m7y = w }
                self.m7_latch = v;
            }
            0x21 => { self.cgadd = v; self.cg_high = false; }
            0x22 => {
                if !self.cg_high {
                    self.cg_latch = v;
                } else {
                    self.cgram[self.cgadd as usize] = self.cg_latch as u16 | ((v & 0x7F) as u16) << 8;
                    self.cgadd = self.cgadd.wrapping_add(1);
                }
                self.cg_high = !self.cg_high;
            }
            0x23 => self.w12sel = v,
            0x24 => self.w34sel = v,
            0x25 => self.wobjsel = v,
            0x26..=0x29 => self.wh[(reg - 0x26) as usize] = v,
            0x2A => self.wbglog = v,
            0x2B => self.wobjlog = v,
            0x2C => self.tm = v,
            0x2D => self.ts = v,
            0x2E => self.tmw = v,
            0x2F => self.tsw = v,
            0x30 => self.cgwsel = v,
            0x31 => self.cgadsub = v,
            0x32 => {
                let c = (v & 0x1F) as u16;
                if v & 0x20 != 0 { self.fixed = (self.fixed & !0x001F) | c; }
                if v & 0x40 != 0 { self.fixed = (self.fixed & !0x03E0) | (c << 5); }
                if v & 0x80 != 0 { self.fixed = (self.fixed & !0x7C00) | (c << 10); }
            }
            _ => {} // TODO: мозаика ($2106), SETINI ($2133): EXTBG, overscan
        }
    }

    pub fn read(&mut self, reg: u8) -> u8 {
        match reg {
            0x38 => {
                let a = self.oam_addr as usize;
                let v = if a & 0x200 != 0 { self.oam[0x200 + (a & 0x1F)] } else { self.oam[a] };
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
                v
            }
            0x39 => {
                let v = self.vram_latch as u8;
                if self.vmain & 0x80 == 0 { self.prefetch(); self.vmadd = self.vmadd.wrapping_add(self.vinc()); }
                v
            }
            0x3A => {
                let v = (self.vram_latch >> 8) as u8;
                if self.vmain & 0x80 != 0 { self.prefetch(); self.vmadd = self.vmadd.wrapping_add(self.vinc()); }
                v
            }
            0x3B => {
                let w = self.cgram[self.cgadd as usize];
                let v = if !self.cg_high { w as u8 } else { self.cgadd = self.cgadd.wrapping_add(1); (w >> 8) as u8 & 0x7F };
                self.cg_high = !self.cg_high;
                v
            }
            // M7A (16 бит, знаковое) × старший байт M7B (8 бит, знаковое)
            0x34..=0x36 => {
                let r = (self.m7[0] as i32) * ((self.m7[1] >> 8) as i8 as i32);
                (r >> ((reg - 0x34) * 8)) as u8
            }
            // счётчики H/V, зафиксированные чтением $2137: сначала младший байт, потом бит 8
            0x3C => {
                let v = if !self.h_toggle { self.hlatch as u8 } else { (self.hlatch >> 8) as u8 & 1 };
                self.h_toggle = !self.h_toggle;
                v
            }
            0x3D => {
                let v = if !self.v_toggle { self.vlatch as u8 } else { (self.vlatch >> 8) as u8 & 1 };
                self.v_toggle = !self.v_toggle;
                v
            }
            0x3E => 0x01,
            0x3F => {
                // чтение сбрасывает переключатели и флаг защёлки
                let v = 0x03 | ((self.latched as u8) << 6);
                self.h_toggle = false;
                self.v_toggle = false;
                self.latched = false;
                v
            }
            _ => 0,
        }
    }

    // ---------- отрисовка ----------
    /// Находится ли пиксель x «внутри» окна слоя (с учётом инверсии и логики двух окон).
    fn window_at(&self, sel: u8, logic: u8, x: usize) -> bool {
        let a = (x >= self.wh[0] as usize && x <= self.wh[1] as usize) != (sel & 1 != 0);
        let b = (x >= self.wh[2] as usize && x <= self.wh[3] as usize) != (sel & 4 != 0);
        match (sel & 2 != 0, sel & 8 != 0) {
            (false, false) => false,
            (true, false) => a,
            (false, true) => b,
            (true, true) => match logic {
                0 => a || b,
                1 => a && b,
                2 => a != b,
                _ => a == b,
            },
        }
    }

    pub fn render_line(&mut self, y: usize) {
        if y >= H { return; }
        if self.inidisp & 0x80 != 0 {
            for x in 0..W {
                let o = (y * W + x) * 4;
                self.fb[o..o + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
            return;
        }
        let mode = (self.bgmode & 7) as usize;
        let en = self.tm | self.ts; // слои нужны и для главного, и для под-экрана
        let mut layers = [[0u16; W]; 5];
        let mut prios = [[0u8; W]; 5];
        if mode == 7 {
            if en & 1 != 0 {
                self.render_mode7(y, &mut layers[0], &mut prios[0]);
            }
        } else if mode <= 4 {
            for bg in 0..4 {
                if en & (1 << bg) != 0 && BPP[mode][bg] != 0 {
                    // мозаика: линия и колонки повторяются блоками size×size
                    let size = ((self.mosaic >> 4) + 1) as usize;
                    let mos = self.mosaic & (1 << bg) != 0 && size > 1;
                    let ye = if mos { y - y % size } else { y };
                    self.render_bg(bg, ye, &mut layers[bg], &mut prios[bg]);
                    if mos {
                        for x in 0..W {
                            let s = x - x % size;
                            layers[bg][x] = layers[bg][s];
                            prios[bg][x] = prios[bg][s];
                        }
                    }
                }
            }
        } // TODO: режимы 5–6 (hires)
        if en & 0x10 != 0 {
            self.render_obj(y, &mut layers[4], &mut prios[4]);
        }
        let order: Order = match mode {
            0 => ORDER_M0,
            7 => ORDER_M7,
            1 => if self.bgmode & 8 != 0 { ORDER_M1_BG3 } else { ORDER_M1 },
            _ => ORDER_M2,
        };

        // маски окон: 0..3 = BG1..BG4, 4 = OBJ, 5 = цветовое окно
        let mut win = [[false; W]; 6];
        for l in 0..6usize {
            let (sel, logic) = match l {
                0..=3 => {
                    let reg = if l < 2 { self.w12sel } else { self.w34sel };
                    ((reg >> ((l & 1) * 4)) & 0xF, (self.wbglog >> (l * 2)) & 3)
                }
                4 => (self.wobjsel & 0xF, self.wobjlog & 3),
                _ => ((self.wobjsel >> 4) & 0xF, (self.wobjlog >> 2) & 3),
            };
            if sel & 0xA == 0 { continue; }
            for x in 0..W {
                win[l][x] = self.window_at(sel, logic, x);
            }
        }

        let bright = (self.inidisp & 15) as u16;
        let ex = |v: u16| -> u8 { let v = v * bright / 15; ((v << 3) | (v >> 2)) as u8 };
        // режимы условий для CGWSEL: 0 никогда, 1 вне окна, 2 внутри окна, 3 всегда
        let gate = |m: u8, cw: bool| match m { 0 => false, 1 => !cw, 2 => cw, _ => true };
        let math_on = self.cgadsub & 0x3F != 0 && (self.cgwsel >> 4) & 3 != 3;

        for x in 0..W {
            let (mi, ml) = pick(order, &layers, &prios, self.tm, self.tmw, &win, x);
            let mut col = if mi == 0 { self.cgram[0] } else { self.cgram[(mi - 1) as usize] };
            let cw = win[5][x];
            if gate((self.cgwsel >> 6) & 3, cw) { col = 0; } // clip to black
            let prevent = gate((self.cgwsel >> 4) & 3, cw);
            // спрайты участвуют в математике только с палитрами 4–7
            if math_on && !prevent && self.cgadsub & (1 << ml) != 0 && (ml != 4 || mi - 1 >= 192) {
                let (sub, sub_backdrop) = if self.cgwsel & 2 != 0 {
                    let (si, _) = pick(order, &layers, &prios, self.ts, self.tsw, &win, x);
                    if si == 0 { (self.fixed, true) } else { (self.cgram[(si - 1) as usize], false) }
                } else {
                    (self.fixed, false)
                };
                // если под-экран пуст (виден фиксированный цвет), деления пополам нет
                let half = self.cgadsub & 0x40 != 0 && !sub_backdrop;
                col = blend(col, sub, self.cgadsub & 0x80 != 0, half);
            }
            let o = (y * W + x) * 4;
            self.fb[o] = ex(col & 31);
            self.fb[o + 1] = ex((col >> 5) & 31);
            self.fb[o + 2] = ex((col >> 10) & 31);
            self.fb[o + 3] = 255;
        }
    }

    /// Записывает в `out` индекс CGRAM + 1 (0 = прозрачно), в `pri` — приоритет тайла.
    fn render_bg(&self, bg: usize, y: usize, out: &mut [u16; W], pri: &mut [u8; W]) {
        let mode = (self.bgmode & 7) as usize;
        let bpp = BPP[mode][bg] as usize;
        let tsz: usize = if self.bgmode & (0x10 << bg) != 0 { 16 } else { 8 };
        let sc = self.bgsc[bg];
        let (hbig, vbig) = (sc & 1 != 0, sc & 2 != 0);
        let map_base = ((sc >> 2) as usize) << 10;
        let chr_base = (((self.bgnba[bg >> 1] >> ((bg & 1) * 4)) & 0xF) as usize) << 12;
        let w_px = tsz * if hbig { 64 } else { 32 };
        let h_px = tsz * if vbig { 64 } else { 32 };
        // первая видимая строка — scanline 1, поэтому VOFS = -1 выравнивает верх (так делает тест-ROM)
        let py = (y + 1 + (self.vofs[bg] & 0x3FF) as usize) % h_px;
        let words_per_tile = bpp * 4;
        for x in 0..W {
            let px = (x + (self.hofs[bg] & 0x3FF) as usize) % w_px;
            let (tx, ty) = (px / tsz, py / tsz);
            let screen = (tx >> 5) + (ty >> 5) * if hbig { 2 } else { 1 };
            let e = self.vram[(map_base + screen * 0x400 + (ty & 31) * 32 + (tx & 31)) & 0x7FFF];
            let (mut fx, mut fy) = (px % tsz, py % tsz);
            if e & 0x4000 != 0 { fx = tsz - 1 - fx; }
            if e & 0x8000 != 0 { fy = tsz - 1 - fy; }
            let mut t = (e & 0x3FF) as usize;
            if tsz == 16 {
                if fx >= 8 { t += 1; fx -= 8; }
                if fy >= 8 { t += 16; fy -= 8; }
            }
            let base = chr_base + t * words_per_tile;
            let mut c = 0usize;
            for pair in 0..bpp / 2 {
                let w = self.vram[(base + pair * 8 + fy) & 0x7FFF];
                let sh = 7 - fx;
                c |= ((((w >> sh) & 1) | (((w >> (8 + sh)) & 1) << 1)) as usize) << (pair * 2);
            }
            if c == 0 { continue; }
            let pal = ((e >> 10) & 7) as usize;
            let idx = match bpp {
                2 => (if mode == 0 { bg * 32 } else { 0 }) + pal * 4 + c,
                4 => pal * 16 + c,
                _ => c,
            };
            out[x] = idx as u16 + 1;
            pri[x] = ((e >> 13) & 1) as u8;
        }
    }

    #[cfg(test)]
    pub fn vram_text(&self, word: usize, len: usize) -> String {
        (0..len).map(|i| (self.vram[word + i] & 0xFF) as u8 as char).collect()
    }

    /// Mode 7: аффинное преобразование поля 1024×1024 (карта 128×128, тайлы 8×8, 8bpp).
    /// Карта лежит в младших байтах слов VRAM, пиксели тайлов — в старших.
    fn render_mode7(&self, y: usize, out: &mut [u16; W], pri: &mut [u8; W]) {
        let clip = |n: i32| if n & 0x2000 != 0 { n | !1023 } else { n & 1023 };
        let [a, b, c, d] = self.m7.map(|v| v as i32);
        let (cx, cy) = (self.m7x as i32, self.m7y as i32);
        let line = (y + 1) as i32;
        let sy = if self.m7sel & 2 != 0 { 255 - line } else { line };
        let dx = clip(self.m7hofs as i32 - cx);
        let dy = clip(self.m7vofs as i32 - cy);
        let ox = ((a * dx) & !63) + ((b * dy) & !63) + ((b * sy) & !63) + (cx << 8);
        let oy = ((c * dx) & !63) + ((d * dy) & !63) + ((d * sy) & !63) + (cy << 8);
        for x in 0..W {
            let sx = if self.m7sel & 1 != 0 { 255 - x as i32 } else { x as i32 };
            let px = (ox + a * sx) >> 8;
            let py = (oy + c * sx) >> 8;
            let outside = ((px | py) & !1023) != 0;
            // M7SEL бит 7-6: 0x = повтор поля, 10 = за полем прозрачно, 11 = за полем тайл 0
            if outside && self.m7sel & 0xC0 == 0x80 { continue; }
            let (tx, ty) = ((px & 1023) as usize, (py & 1023) as usize);
            let tile = if outside && self.m7sel & 0xC0 == 0xC0 {
                0
            } else {
                (self.vram[((ty >> 3) * 128 + (tx >> 3)) & 0x7FFF] & 0xFF) as usize
            };
            let col = (self.vram[(tile * 64 + (ty & 7) * 8 + (tx & 7)) & 0x7FFF] >> 8) as usize;
            if col == 0 { continue; }
            out[x] = col as u16 + 1;
            pri[x] = 0;
        }
    }

    fn render_obj(&self, y: usize, out: &mut [u16; W], pri: &mut [u8; W]) {
        let sizes = OBJ_SIZES[(self.obsel >> 5) as usize];
        let name_base = ((self.obsel & 7) as usize) << 13;
        let gap = (((self.obsel >> 3) & 3) as usize + 1) << 12;
        let hi_bits = |i: usize| (self.oam[512 + i / 4] >> ((i & 3) * 2)) & 3;

        // до 32 спрайтов на строку; меньший номер = выше приоритет
        let mut found = [0usize; 32];
        let mut n = 0;
        for i in 0..128 {
            let (_, h) = sizes[(hi_bits(i) >> 1) as usize];
            let dy = (y + 256 - (self.oam[i * 4 + 1] as usize + 1)) & 0xFF;
            if dy < h {
                if n == 32 { break; }
                found[n] = i;
                n += 1;
            }
        }
        for &i in found[..n].iter().rev() {
            let b = i * 4;
            let hi = hi_bits(i);
            let (w, h) = sizes[(hi >> 1) as usize];
            let mut x = self.oam[b] as i32 | ((hi & 1) as i32) << 8;
            if x >= 256 { x -= 512; }
            let tile = self.oam[b + 2] as usize;
            let attr = self.oam[b + 3];
            let mut dy = (y + 256 - (self.oam[b + 1] as usize + 1)) & 0xFF;
            if attr & 0x80 != 0 { dy = h - 1 - dy; }
            let pal = ((attr >> 1) & 7) as usize;
            let prio = (attr >> 4) & 3;
            let base = name_base + if attr & 1 != 0 { gap } else { 0 };
            for px in 0..w {
                let sx = x + px as i32;
                if !(0..256).contains(&sx) { continue; }
                let fx = if attr & 0x40 != 0 { w - 1 - px } else { px };
                let t = ((((tile >> 4) + (dy >> 3)) & 0xF) << 4) | (((tile & 0xF) + (fx >> 3)) & 0xF);
                let a = base + t * 16;
                let sh = 7 - (fx & 7);
                let mut c = 0usize;
                for pair in 0..2 {
                    let wd = self.vram[(a + pair * 8 + (dy & 7)) & 0x7FFF];
                    c |= ((((wd >> sh) & 1) | (((wd >> (8 + sh)) & 1) << 1)) as usize) << (pair * 2);
                }
                if c == 0 { continue; }
                out[sx as usize] = (128 + pal * 16 + c) as u16 + 1;
                pri[sx as usize] = prio;
            }
        }
    }
}

/// Верхний видимый пиксель по порядку слоёв: (индекс CGRAM + 1 или 0, слой; 5 = фон).
/// `en` — включённые слои (TM/TS), `wen` — слои, маскируемые окном (TMW/TSW).
fn pick(order: Order, layers: &[[u16; W]; 5], prios: &[[u8; W]; 5], en: u8, wen: u8, win: &[[bool; W]; 6], x: usize) -> (u16, usize) {
    for &(l, p) in order {
        let l = l as usize;
        if en & (1 << l) == 0 { continue; }
        let i = layers[l][x];
        if i == 0 || prios[l][x] != p { continue; }
        if wen & (1 << l) != 0 && win[l][x] { continue; }
        return (i, l);
    }
    (0, 5)
}

/// Цветовая математика поканально (BGR555): сложение или вычитание, опционально /2.
fn blend(a: u16, b: u16, subtract: bool, half: bool) -> u16 {
    let mut out = 0u16;
    for s in [0u16, 5, 10] {
        let x = ((a >> s) & 31) as i32;
        let y = ((b >> s) & 31) as i32;
        let mut r = if subtract { x - y } else { x + y };
        if half { r >>= 1; }
        out |= (r.clamp(0, 31) as u16) << s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg1_tile_renders() {
        let mut p = Ppu::new();
        // CGRAM[1] = красный (BGR555 = 0x001F)
        p.write(0x21, 1);
        p.write(0x22, 0x1F);
        p.write(0x22, 0x00);
        // 4bpp-тайл №1 (слово 16): строка 0, плоскость 0 = 0x80 → пиксель x=0 цвета 1
        p.write(0x15, 0x80);
        p.write(0x16, 16);
        p.write(0x17, 0);
        p.write(0x18, 0x80);
        p.write(0x19, 0x00);
        // карта BG1 на слове 0x400, ячейка (0,0) = тайл 1
        p.write(0x07, 0x04);
        p.write(0x16, 0x00);
        p.write(0x17, 0x04);
        p.write(0x18, 0x01);
        p.write(0x19, 0x00);
        p.write(0x05, 0x01); // mode 1
        p.write(0x0E, 0xFF); // BG1VOFS = -1, как в тест-ROM
        p.write(0x0E, 0xFF);
        p.write(0x2C, 0x01); // BG1 на главном экране
        p.write(0x00, 0x0F); // яркость 15
        p.render_line(0);
        assert_eq!(&p.fb[0..4], &[255, 0, 0, 255]);
        assert_eq!(&p.fb[4..8], &[0, 0, 0, 255]); // x=1 — фон (CGRAM[0] = 0)
    }

    #[test]
    fn color_math_adds_fixed_color_to_backdrop() {
        let mut p = Ppu::new();
        p.write(0x00, 0x0F);
        p.write(0x31, 0x20); // математика для фона, сложение
        p.write(0x32, 0x40 | 31); // фиксированный цвет: зелёный = 31
        p.render_line(0);
        assert_eq!(&p.fb[0..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn half_and_subtract() {
        assert_eq!(blend(0x001F, 0x001F, false, true), 0x001F); // (31+31)/2
        assert_eq!(blend(0x0004, 0x0010, true, false), 0); // не уходит ниже 0
        assert_eq!(blend(0x0010, 0x0004, true, false), 0x000C);
    }

    #[test]
    fn window_ranges_and_invert() {
        let mut p = Ppu::new();
        p.wh = [10, 20, 0, 0];
        assert!(p.window_at(0b0010, 0, 15)); // окно 1 включено
        assert!(!p.window_at(0b0010, 0, 5));
        assert!(p.window_at(0b0011, 0, 5)); // инверсия
        assert!(!p.window_at(0b0000, 0, 15)); // окна выключены
    }

    #[test]
    fn mode7_pixel_and_multiply() {
        let mut p = Ppu::new();
        p.write(0x15, 0x80);
        p.write(0x16, 0);
        p.write(0x17, 0);
        p.write(0x18, 1); // карта: ячейка 0 = тайл 1
        p.write(0x19, 0);
        p.write(0x16, 64);
        p.write(0x17, 0);
        p.write(0x18, 0);
        p.write(0x19, 5); // пиксель (0,0) тайла 1 = цвет 5
        p.write(0x21, 5);
        p.write(0x22, 0x00);
        p.write(0x22, 0x7C); // CGRAM[5] = синий
        for (reg, lo, hi) in [(0x1B, 0x00, 0x01), (0x1C, 0, 0), (0x1D, 0, 0), (0x1E, 0x00, 0x01)] {
            p.write(reg, lo);
            p.write(reg, hi); // матрица = единичная (1.0 = 0x0100)
        }
        p.write(0x0E, 0xFF);
        p.write(0x0E, 0x1F); // M7VOFS = -1: строка 0 попадает на y = 0 поля
        p.write(0x05, 0x07);
        p.write(0x2C, 0x01);
        p.write(0x00, 0x0F);
        p.render_line(0);
        assert_eq!(&p.fb[0..4], &[0, 0, 255, 255]);
        assert_eq!(&p.fb[4..8], &[0, 0, 0, 255]);

        // умножение: M7A = 3, старший байт M7B = 5 → 15
        p.write(0x1B, 3);
        p.write(0x1B, 0);
        p.write(0x1C, 0);
        p.write(0x1C, 5);
        assert_eq!((p.read(0x34), p.read(0x35), p.read(0x36)), (15, 0, 0));
    }

    #[test]
    fn hv_latch_reads_low_then_high() {
        let mut p = Ppu::new();
        p.latch_counters(300, 100);
        assert_eq!((p.read(0x3C), p.read(0x3C)), (0x2C, 1));
        assert_eq!((p.read(0x3D), p.read(0x3D)), (100, 0));
        assert_eq!(p.read(0x3F) & 0x40, 0x40);
        assert_eq!(p.read(0x3F) & 0x40, 0);
    }

    #[test]
    fn horizontal_scroll_keeps_all_low_bits() {
        // игра пишет BGnHOFS дважды (младший, старший байт); раньше терялся бит 2
        let mut p = Ppu::new();
        for x in [5u16, 4, 12, 13, 0xFF, 0x1FD, 0x3FF, 1, 8] {
            p.write(0x0D, x as u8);
            p.write(0x0D, (x >> 8) as u8);
            assert_eq!(p.hofs[0] & 0x3FF, x, "scroll = {}", x);
        }
        // равномерный скролл на 1 пиксель за кадр
        for x in 0..64u16 {
            p.write(0x0D, x as u8);
            p.write(0x0D, 0);
            assert_eq!(p.hofs[0] & 0x3FF, x);
        }
    }

    #[test]
    fn oam_write_pairs() {
        let mut p = Ppu::new();
        p.write(0x02, 0);
        p.write(0x03, 0);
        for v in [10u8, 20, 30, 40] { p.write(0x04, v); }
        assert_eq!(&p.oam[0..4], &[10, 20, 30, 40]);
    }
}
