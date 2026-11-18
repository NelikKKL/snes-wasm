use crate::cartridge::Cartridge;
use crate::apu::Apu;
use crate::dma::Channel;
use crate::ppu::Ppu;

pub struct Bus {
    pub cart: Cartridge,
    pub wram: Vec<u8>,
    // джойпад
    pub joy1: u16, // текущее состояние кнопок (формат JOY1, см. lib.rs)
    pub joy1_latched: u16,
    pub joy1_shift: u16,
    pub strobe: bool,
    pub joy_busy: bool,
    // $42xx
    pub nmitimen: u8,
    pub nmi_flag: bool,    // $4210 бит 7
    pub nmi_trigger: bool, // фронт NMI, забирается эмулятором
    pub timeup: bool,      // $4211 бит 7 (линия IRQ)
    pub htime: u16,
    pub vtime: u16,
    pub vblank: bool,
    pub hclock: u32, // master-такт внутри строки на начало инструкции
    pub wrmpya: u8,
    pub wrdiv: u16,
    pub rddiv: u16,
    pub rdmpy: u16,
    pub hdmaen: u8,
    pub memsel: u8,
    pub wram_addr: u32,
    pub apu: Apu,
    pub dma: [Channel; 8],
    pub stall: u32, // master-такты, потраченные DMA
    pub ppu: Ppu,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Bus {
            cart, wram: vec![0; 0x20000],
            joy1: 0, joy1_latched: 0, joy1_shift: 0, strobe: false, joy_busy: false,
            nmitimen: 0, nmi_flag: false, nmi_trigger: false, timeup: false,
            htime: 0x1FF, vtime: 0x1FF, vblank: false, hclock: 0,
            wrmpya: 0xFF, wrdiv: 0xFFFF, rddiv: 0, rdmpy: 0,
            hdmaen: 0, memsel: 0, wram_addr: 0,
            apu: Apu::new(),
            dma: [Channel::default(); 8],
            stall: 0,
            ppu: Ppu::new(),
        }
    }

    /// Стоимость обращения к адресу в master-тактах (6 / 8 / 12).
    pub fn cost(&self, addr: u32) -> u32 {
        let bank = (addr >> 16) as u8;
        let off = addr as u16;
        let fast = self.memsel & 1 != 0;
        match bank {
            0x40..=0x7F => 8,
            0xC0..=0xFF => if fast { 6 } else { 8 },
            _ => match off {
                0x0000..=0x1FFF => 8,
                0x2000..=0x3FFF => 6,
                0x4000..=0x41FF => 12,
                0x4200..=0x5FFF => 6,
                0x6000..=0x7FFF => 8,
                _ => if bank >= 0x80 && fast { 6 } else { 8 },
            },
        }
    }

    pub fn read(&mut self, addr: u32) -> u8 {
        let bank = (addr >> 16) as u8;
        let off = addr as u16;
        match (bank, off) {
            (0x7E, _) => self.wram[off as usize],
            (0x7F, _) => self.wram[0x10000 + off as usize],
            (0x00..=0x3F | 0x80..=0xBF, 0x0000..=0x1FFF) => self.wram[off as usize],
            (0x00..=0x3F | 0x80..=0xBF, 0x2000..=0x5FFF) => self.read_io(off),
            _ => self.cart.read(bank, off),
        }
    }

    pub fn write(&mut self, addr: u32, val: u8) {
        let bank = (addr >> 16) as u8;
        let off = addr as u16;
        match (bank, off) {
            (0x7E, _) => self.wram[off as usize] = val,
            (0x7F, _) => self.wram[0x10000 + off as usize] = val,
            (0x00..=0x3F | 0x80..=0xBF, 0x0000..=0x1FFF) => self.wram[off as usize] = val,
            (0x00..=0x3F | 0x80..=0xBF, 0x2000..=0x5FFF) => self.write_io(off, val),
            _ => self.cart.write(bank, off, val),
        }
    }

    fn read_io(&mut self, off: u16) -> u8 {
        match off {
            0x2100..=0x21FF => self.b_read((off & 0xFF) as u8),
            0x4016 => {
                if self.strobe { self.joy1_shift = self.joy1; }
                let bit = (self.joy1_shift >> 15) as u8 & 1;
                if !self.strobe { self.joy1_shift = (self.joy1_shift << 1) | 1; }
                bit
            }
            0x4017 => 0x1C,
            0x4210 => { let v = (self.nmi_flag as u8) << 7 | 0x02; self.nmi_flag = false; v }
            0x4211 => { let v = (self.timeup as u8) << 7; self.timeup = false; v }
            0x4212 => (self.vblank as u8) << 7 | ((self.hclock >= 1096) as u8) << 6 | self.joy_busy as u8,
            0x4213 => 0xFF,
            0x4214 => self.rddiv as u8,
            0x4215 => (self.rddiv >> 8) as u8,
            0x4216 => self.rdmpy as u8,
            0x4217 => (self.rdmpy >> 8) as u8,
            0x4218 => self.joy1_latched as u8,
            0x4219 => (self.joy1_latched >> 8) as u8,
            0x4300..=0x437F => self.dma_read(off),
            _ => 0,
        }
    }

    fn write_io(&mut self, off: u16, v: u8) {
        match off {
            0x2100..=0x21FF => self.b_write((off & 0xFF) as u8, v),
            0x4016 => {
                let was = self.strobe;
                self.strobe = v & 1 != 0;
                if was && !self.strobe { self.joy1_shift = self.joy1; }
            }
            0x4200 => {
                let old = self.nmitimen;
                self.nmitimen = v;
                // включение NMI во время VBlank при поднятом флаге даёт NMI сразу
                if old & 0x80 == 0 && v & 0x80 != 0 && self.nmi_flag { self.nmi_trigger = true; }
                if v & 0x30 == 0 { self.timeup = false; }
            }
            0x4202 => self.wrmpya = v,
            0x4203 => self.rdmpy = self.wrmpya as u16 * v as u16,
            0x4204 => self.wrdiv = (self.wrdiv & 0xFF00) | v as u16,
            0x4205 => self.wrdiv = (self.wrdiv & 0x00FF) | (v as u16) << 8,
            0x4206 => {
                if v == 0 {
                    self.rddiv = 0xFFFF;
                    self.rdmpy = self.wrdiv;
                } else {
                    self.rddiv = self.wrdiv / v as u16;
                    self.rdmpy = self.wrdiv % v as u16;
                }
            }
            0x4207 => self.htime = (self.htime & 0x100) | v as u16,
            0x4208 => self.htime = (self.htime & 0x0FF) | ((v & 1) as u16) << 8,
            0x4209 => self.vtime = (self.vtime & 0x100) | v as u16,
            0x420A => self.vtime = (self.vtime & 0x0FF) | ((v & 1) as u16) << 8,
            0x420B => { let c = self.run_gdma(v); self.stall += c; }
            0x420C => self.hdmaen = v,
            0x420D => self.memsel = v,
            0x4300..=0x437F => self.dma_write(off, v),
            _ => {}
        }
    }

    /// B-шина ($2100–$21FF): регистры PPU/APU/WRAM-порт.
    pub fn b_read(&mut self, reg: u8) -> u8 {
        match reg {
            0x40..=0x7F => self.apu.port_read((reg & 3) as usize),
            0x80 => {
                let v = self.wram[self.wram_addr as usize];
                self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
                v
            }
            _ => self.ppu.read(reg),
        }
    }

    pub fn b_write(&mut self, reg: u8, v: u8) {
        match reg {
            0x40..=0x7F => self.apu.port_write((reg & 3) as usize, v),
            0x80 => {
                let a = self.wram_addr as usize;
                self.wram[a] = v;
                self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
            }
            0x81 => self.wram_addr = (self.wram_addr & 0x1FF00) | v as u32,
            0x82 => self.wram_addr = (self.wram_addr & 0x100FF) | (v as u32) << 8,
            0x83 => self.wram_addr = (self.wram_addr & 0x0FFFF) | ((v & 1) as u32) << 16,
            _ => self.ppu.write(reg, v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> Bus {
        Bus::new(Cartridge::from_bytes(&vec![0u8; 0x8000]).unwrap())
    }

    #[test]
    fn mul_div() {
        let mut b = bus();
        b.write(0x4202, 7);
        b.write(0x4203, 6);
        assert_eq!(b.read(0x4216), 42);
        b.write(0x4204, 100);
        b.write(0x4205, 0);
        b.write(0x4206, 7);
        assert_eq!((b.read(0x4214), b.read(0x4216)), (14, 2));
    }

    #[test]
    fn nmi_flag_clears_on_read() {
        let mut b = bus();
        b.nmi_flag = true;
        assert_eq!(b.read(0x4210), 0x82);
        assert_eq!(b.read(0x4210), 0x02);
    }

    #[test]
    fn gdma_to_wram_port() {
        let mut b = bus();
        b.wram[0x100..0x104].copy_from_slice(&[1, 2, 3, 4]);
        // WMADD = 0x0200; канал 0: A→B, режим 0, B = $2180, A = 00:0100, 4 байта
        for (r, v) in [
            (0x2181u32, 0x00u8), (0x2182, 0x02), (0x2183, 0x00),
            (0x4300, 0x00), (0x4301, 0x80), (0x4302, 0x00), (0x4303, 0x01),
            (0x4304, 0x00), (0x4305, 0x04), (0x4306, 0x00), (0x420B, 0x01),
        ] {
            b.write(r, v);
        }
        assert_eq!(&b.wram[0x200..0x204], &[1, 2, 3, 4]);
        assert!(b.stall > 0);
    }
}
