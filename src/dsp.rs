//! S-DSP: 8 голосов (BRR), огибающие ADSR/GAIN, шум, модуляция тона и эхо с FIR.
//! Таблица Гаусса строится по формуле — это приближение таблицы из ROM консоли.

const RATES: [u16; 32] = [
    0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80,
    64, 48, 40, 32, 24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1,
];

/// Срабатывает ли счётчик скоростей `counter` для `rate` (0 = никогда).
fn rate_tick(counter: u16, rate: u8) -> bool {
    if rate == 0 {
        return false;
    }
    let offset: u32 = if rate == 1 { 0 } else { [1040, 536, 0][((rate - 2) % 3) as usize] };
    (counter as u32 + offset) % RATES[rate as usize] as u32 == 0
}

fn clamp16(v: i32) -> i32 {
    v.clamp(-32768, 32767)
}

#[derive(Clone, Copy, Default)]
struct Voice {
    addr: u16,      // адрес текущего BRR-блока
    header: u8,     // заголовок текущего блока
    buf: [i16; 16], // декодированные отсчёты блока (в железной «удвоенной» шкале)
    prev: [i16; 3], // последние 3 отсчёта предыдущего блока (для интерполяции)
    idx: usize,     // позиция в блоке
    counter: u32,   // дробная часть позиции, 0x1000 = один отсчёт
    env: i32,
    mode: u8,       // 0 attack, 1 decay, 2 sustain, 3 release
    on: bool,
    out: i32,
}

pub struct Dsp {
    regs: [u8; 128],
    v: [Voice; 8],
    counter: u16, // глобальный счётчик скоростей: 0x77FF → 0
    noise: i32,
    endx: u8,
    kon: u8,
    echo_pos: u32,
    fir_l: [i32; 8],
    fir_r: [i32; 8],
    fir_pos: usize,
    gauss: [i16; 512],
}

impl Dsp {
    pub fn new() -> Self {
        // вес отсчёта по расстоянию d (в отсчётах): гауссиана σ≈0.63, обнулённая при d = 2
        let mut gauss = [0i16; 512];
        let sigma2 = 0.63f64 * 0.63;
        let edge = (-4.0 / (2.0 * sigma2)).exp();
        for (n, g) in gauss.iter_mut().enumerate() {
            let d = ((511.0 - n as f64) / 256.0).clamp(0.0, 2.0);
            let k = ((-(d * d) / (2.0 * sigma2)).exp() - edge) / (1.0 - edge);
            *g = (1316.0 * k).round() as i16;
        }
        let mut regs = [0u8; 128];
        regs[0x6C] = 0xE0; // FLG после сброса: reset + mute + запрет записи эха
        Dsp {
            regs, v: [Voice::default(); 8], counter: 0x77FF, noise: 0x4000, endx: 0, kon: 0,
            echo_pos: 0, fir_l: [0; 8], fir_r: [0; 8], fir_pos: 0, gauss,
        }
    }

    pub fn read(&self, addr: u8) -> u8 {
        if addr & 0x7F == 0x7C { self.endx } else { self.regs[(addr & 0x7F) as usize] }
    }

    pub fn write(&mut self, addr: u8, v: u8) {
        let a = (addr & 0x7F) as usize;
        match a {
            0x4C => { self.regs[a] = v; self.kon |= v; }
            0x7C => self.endx = 0, // любая запись сбрасывает ENDX
            _ => self.regs[a] = v,
        }
    }

    fn decode_block(v: &mut Voice, ram: &[u8]) {
        let a = v.addr as usize;
        let header = ram[a];
        v.header = header;
        let range = (header >> 4) as u32;
        let filter = (header >> 2) & 3;
        v.prev = [v.buf[13], v.buf[14], v.buf[15]];
        let (mut p1, mut p2) = (v.buf[15] as i32, v.buf[14] as i32);
        for n in 0..16 {
            let byte = ram[(a + 1 + n / 2) & 0xFFFF];
            let nib = (if n % 2 == 0 { byte >> 4 } else { byte & 0xF }) as i32;
            let mut s = (nib ^ 8) - 8;
            s = if range <= 12 { (s << range) >> 1 } else if s < 0 { -2048 } else { 0 };
            let (a1, a2) = (p1, p2 >> 1);
            match filter {
                0 => {}
                1 => { s += a1 >> 1; s += (-a1) >> 5; }
                2 => { s += a1; s -= a2; s += a2 >> 4; s += (a1 * -3) >> 6; }
                _ => { s += a1; s -= a2; s += (a1 * -13) >> 7; s += (a2 * 3) >> 4; }
            }
            s = clamp16(s);
            s = ((s << 1) as i16) as i32;
            v.buf[n] = s as i16;
            p2 = p1;
            p1 = s;
        }
    }

    fn key_on(&mut self, i: usize, ram: &[u8], dir: usize) {
        let srcn = self.regs[i * 16 + 4] as usize;
        let e = (dir + srcn * 4) & 0xFFFF;
        let start = ram[e] as u16 | (ram[(e + 1) & 0xFFFF] as u16) << 8;
        let v = &mut self.v[i];
        *v = Voice::default();
        v.addr = start;
        v.on = true;
        Self::decode_block(v, ram);
        self.endx &= !(1u8 << i);
    }

    fn run_env(&mut self, i: usize) {
        let base = i * 16;
        let (a1, a2, gain) = (self.regs[base + 5], self.regs[base + 6], self.regs[base + 7]);
        let c = self.counter;
        let v = &mut self.v[i];
        if v.mode == 3 {
            v.env = (v.env - 8).max(0);
            return;
        }
        if a1 & 0x80 != 0 {
            match v.mode {
                0 => {
                    let rate = (a1 & 0x0F) * 2 + 1;
                    if rate_tick(c, rate) {
                        v.env += if rate == 31 { 1024 } else { 32 };
                        if v.env > 0x7FF { v.env = 0x7FF; v.mode = 1; }
                    }
                }
                1 => {
                    let rate = ((a1 >> 4) & 7) * 2 + 16;
                    if rate_tick(c, rate) { v.env -= ((v.env - 1) >> 8) + 1; }
                    if (v.env >> 8) <= (a2 >> 5) as i32 { v.mode = 2; }
                }
                _ => {
                    if rate_tick(c, a2 & 0x1F) { v.env -= ((v.env - 1) >> 8) + 1; }
                }
            }
        } else if gain & 0x80 == 0 {
            v.env = ((gain & 0x7F) as i32) << 4;
        } else if rate_tick(c, gain & 0x1F) {
            v.env = match (gain >> 5) & 3 {
                0 => v.env - 32,
                1 => v.env - (((v.env - 1) >> 8) + 1),
                2 => v.env + 32,
                _ => v.env + if v.env < 0x600 { 32 } else { 8 },
            };
        }
        v.env = v.env.clamp(0, 0x7FF);
    }

    fn sample_at(v: &Voice, back: usize) -> i32 {
        if v.idx >= back { v.buf[v.idx - back] as i32 } else { v.prev[3 - (back - v.idx)] as i32 }
    }

    /// Гауссова интерполяция по четырём последним отсчётам.
    fn interpolate(&self, v: &Voice) -> i32 {
        let i = ((v.counter >> 4) & 0xFF) as usize;
        let g = &self.gauss;
        let mut out = (g[255 - i] as i32 * Self::sample_at(v, 3)) >> 11;
        out += (g[511 - i] as i32 * Self::sample_at(v, 2)) >> 11;
        out += (g[256 + i] as i32 * Self::sample_at(v, 1)) >> 11;
        out = (out as i16) as i32;
        out += (g[i] as i32 * Self::sample_at(v, 0)) >> 11;
        clamp16(out) & !1
    }

    /// Сдвигает позицию воспроизведения голоса, подгружает следующие BRR-блоки.
    fn advance(&mut self, i: usize, pitch: u32, ram: &[u8], dir: usize) {
        let srcn = self.regs[i * 16 + 4] as usize;
        let v = &mut self.v[i];
        v.counter += pitch;
        v.idx += (v.counter >> 12) as usize;
        v.counter &= 0xFFF;
        while v.idx >= 16 {
            v.idx -= 16;
            if v.header & 1 != 0 {
                self.endx |= 1 << i;
                if v.header & 2 != 0 {
                    let e = (dir + srcn * 4 + 2) & 0xFFFF;
                    v.addr = ram[e] as u16 | (ram[(e + 1) & 0xFFFF] as u16) << 8;
                } else {
                    v.on = false; // конец без петли — голос замолкает
                    v.env = 0;
                    return;
                }
            } else {
                v.addr = v.addr.wrapping_add(9);
            }
            Self::decode_block(v, ram);
        }
    }

    /// Один стерео-отсчёт (32 кГц).
    pub fn sample(&mut self, ram: &mut [u8]) -> (i16, i16) {
        self.counter = if self.counter == 0 { 0x77FF } else { self.counter - 1 };
        let flg = self.regs[0x6C];
        if flg & 0x80 != 0 {
            for v in self.v.iter_mut() { v.on = false; v.mode = 3; v.env = 0; }
            self.kon = 0;
        }
        if rate_tick(self.counter, flg & 0x1F) {
            self.noise = (((self.noise << 13) ^ (self.noise << 14)) & 0x4000) | (self.noise >> 1);
        }
        let dir = (self.regs[0x5D] as usize) << 8;
        let (mut ml, mut mr, mut el, mut er) = (0i32, 0i32, 0i32, 0i32);
        let mut prev_out = 0i32;

        for i in 0..8 {
            let bit = 1u8 << i;
            let base = i * 16;
            if self.kon & bit != 0 {
                self.key_on(i, ram, dir);
            } else if self.regs[0x5C] & bit != 0 {
                self.v[i].mode = 3;
            }
            if !self.v[i].on {
                self.regs[base + 8] = 0;
                self.regs[base + 9] = 0;
                prev_out = 0;
                continue;
            }
            self.run_env(i);
            let out = {
                let v = &self.v[i];
                let s = if self.regs[0x3D] & bit != 0 { ((self.noise << 1) as i16) as i32 } else { self.interpolate(v) };
                (s * v.env) >> 11
            };
            self.v[i].out = out;
            self.regs[base + 8] = (self.v[i].env >> 4) as u8;
            self.regs[base + 9] = (out >> 8) as u8;
            let (vl, vr) = (self.regs[base] as i8 as i32, self.regs[base + 1] as i8 as i32);
            let (l, r) = ((out * vl) >> 7, (out * vr) >> 7);
            ml = clamp16(ml + l);
            mr = clamp16(mr + r);
            if self.regs[0x4D] & bit != 0 {
                el = clamp16(el + l);
                er = clamp16(er + r);
            }
            let mut pitch = self.regs[base + 2] as i32 | ((self.regs[base + 3] as i32 & 0x3F) << 8);
            if self.regs[0x2D] & bit != 0 && i > 0 {
                pitch += ((prev_out >> 5) * pitch) >> 10;
            }
            prev_out = out;
            self.advance(i, pitch.clamp(0, 0x3FFF) as u32, ram, dir);
        }
        self.kon = 0;

        // ----- эхо -----
        let esa = (self.regs[0x6D] as usize) << 8;
        let edl = (self.regs[0x7D] & 0x0F) as u32;
        let len: u32 = if edl == 0 { 4 } else { edl * 0x800 };
        let a = esa + self.echo_pos as usize;
        let rd = |ram: &[u8], a: usize| -> i32 {
            (ram[a & 0xFFFF] as u16 | (ram[(a + 1) & 0xFFFF] as u16) << 8) as i16 as i32
        };
        self.fir_l[self.fir_pos] = rd(ram, a) >> 1;
        self.fir_r[self.fir_pos] = rd(ram, a + 2) >> 1;
        let coef: [i32; 8] = std::array::from_fn(|k| self.regs[0x0F + k * 0x10] as i8 as i32);
        let fir = |h: &[i32; 8], pos: usize| -> i32 {
            let mut sum = 0i32;
            for k in 0..7 {
                sum += (h[(pos + 1 + k) & 7] * coef[k]) >> 6;
            }
            sum = (sum as i16) as i32;
            clamp16(sum + ((h[(pos + 8) & 7] * coef[7]) >> 6))
        };
        let fl = fir(&self.fir_l, self.fir_pos);
        let fr = fir(&self.fir_r, self.fir_pos);
        self.fir_pos = (self.fir_pos + 1) & 7;

        let (mvl, mvr) = (self.regs[0x0C] as i8 as i32, self.regs[0x1C] as i8 as i32);
        let (evl, evr) = (self.regs[0x2C] as i8 as i32, self.regs[0x3C] as i8 as i32);
        let out_l = clamp16(clamp16((ml * mvl) >> 7) + clamp16((fl * evl) >> 7));
        let out_r = clamp16(clamp16((mr * mvr) >> 7) + clamp16((fr * evr) >> 7));

        if flg & 0x20 == 0 {
            let efb = self.regs[0x0D] as i8 as i32;
            let wl = clamp16(el + ((fl * efb) >> 7)) & !1;
            let wr = clamp16(er + ((fr * efb) >> 7)) & !1;
            ram[a & 0xFFFF] = wl as u8;
            ram[(a + 1) & 0xFFFF] = (wl >> 8) as u8;
            ram[(a + 2) & 0xFFFF] = wr as u8;
            ram[(a + 3) & 0xFFFF] = (wr >> 8) as u8;
        }
        self.echo_pos += 4;
        if self.echo_pos >= len {
            self.echo_pos = 0;
        }
        if flg & 0x40 != 0 { (0, 0) } else { (out_l as i16, out_r as i16) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brr_filter0_decodes_nibbles() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x200] = 0xC3; // range 12, filter 0, loop+end
        ram[0x201] = 0x70; // нибблы 7 и 0
        let mut v = Voice::default();
        v.addr = 0x200;
        Dsp::decode_block(&mut v, &ram);
        assert_eq!(v.buf[0], 0x7000); // (7 << 12) >> 1 = 0x3800, затем удвоение
        assert_eq!(v.buf[1], 0);
    }

    #[test]
    fn looped_flat_sample_makes_sound() {
        let mut ram = vec![0u8; 0x10000];
        // DIR = $0100: запись 0 = {старт $0200, петля $0200}
        ram[0x100..0x104].copy_from_slice(&[0x00, 0x02, 0x00, 0x02]);
        ram[0x200] = 0xC3;
        for k in 1..9 { ram[0x200 + k] = 0x77; }
        let mut d = Dsp::new();
        for (a, v) in [
            (0x6Cu8, 0x20u8), (0x5D, 0x01), (0x0C, 0x7F), (0x1C, 0x7F),
            (0x00, 0x7F), (0x01, 0x7F), (0x02, 0x00), (0x03, 0x10),
            (0x04, 0x00), (0x05, 0x00), (0x07, 0x7F), (0x4C, 0x01),
        ] {
            d.write(a, v);
        }
        let mut last = (0, 0);
        for _ in 0..200 { last = d.sample(&mut ram); }
        assert!(last.0 > 20000 && last.0 == last.1, "{:?}", last);
    }

    #[test]
    fn key_off_decays_to_silence() {
        let mut ram = vec![0u8; 0x10000];
        ram[0x100..0x104].copy_from_slice(&[0x00, 0x02, 0x00, 0x02]);
        ram[0x200] = 0xC3;
        for k in 1..9 { ram[0x200 + k] = 0x77; }
        let mut d = Dsp::new();
        for (a, v) in [
            (0x6Cu8, 0x20u8), (0x5D, 0x01), (0x0C, 0x7F), (0x1C, 0x7F), (0x00, 0x7F), (0x01, 0x7F),
            (0x03, 0x10), (0x07, 0x7F), (0x4C, 0x01),
        ] {
            d.write(a, v);
        }
        for _ in 0..50 { d.sample(&mut ram); }
        d.write(0x5C, 0x01); // KOFF
        let mut last = (1, 1);
        for _ in 0..400 { last = d.sample(&mut ram); }
        assert_eq!(last, (0, 0));
    }
}
