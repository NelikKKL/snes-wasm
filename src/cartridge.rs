#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mapping {
    LoRom,
    HiRom,
}

pub struct Cartridge {
    pub rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub mapping: Mapping,
    pub title: String,
}

/// Оценка правдоподобности заголовка по смещению `base` (0x7FC0 или 0xFFC0).
fn score(rom: &[u8], base: usize, hirom: bool) -> i32 {
    if rom.len() < base + 0x40 {
        return -100;
    }
    let h = &rom[base..base + 0x40];
    let comp = u16::from_le_bytes([h[0x1C], h[0x1D]]);
    let sum = u16::from_le_bytes([h[0x1E], h[0x1F]]);
    let reset = u16::from_le_bytes([h[0x3C], h[0x3D]]);
    let mut s = 0;
    if comp ^ sum == 0xFFFF {
        s += 4;
    }
    if (h[0x15] & 1 == 1) == hirom {
        s += 2;
    }
    if reset >= 0x8000 {
        s += 2;
    }
    if h[..21].iter().all(|&c| (0x20..0x7F).contains(&c)) {
        s += 1;
    }
    s
}

impl Cartridge {
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        // Копир-заголовок 512 байт (.smc)
        let rom = if data.len() % 0x400 == 0x200 { &data[0x200..] } else { data };
        if rom.len() < 0x8000 {
            return Err("Файл слишком мал для SNES ROM".into());
        }
        let lo = score(rom, 0x7FC0, false);
        let hi = score(rom, 0xFFC0, true);
        let (mapping, base) = if hi > lo { (Mapping::HiRom, 0xFFC0) } else { (Mapping::LoRom, 0x7FC0) };
        let h = &rom[base..base + 0x40];
        let title = String::from_utf8_lossy(&h[..21]).trim().to_string();
        let sram_kb = if h[0x18] == 0 { 0 } else { 1usize << h[0x18].min(8) };
        Ok(Cartridge { rom: rom.to_vec(), sram: vec![0; sram_kb * 1024], mapping, title })
    }

    fn sram_index(&self, bank: u8, off: u16) -> Option<usize> {
        if self.sram.is_empty() {
            return None;
        }
        let (b, o) = (bank as usize & 0x7F, off as usize);
        let idx = match self.mapping {
            Mapping::LoRom if (0x70..=0x7D).contains(&b) && o < 0x8000 => (b - 0x70) * 0x8000 + o,
            Mapping::HiRom if (0x20..=0x3F).contains(&b) && (0x6000..0x8000).contains(&o) => {
                (b - 0x20) * 0x2000 + (o - 0x6000)
            }
            _ => return None,
        };
        Some(idx % self.sram.len())
    }

    pub fn read(&self, bank: u8, off: u16) -> u8 {
        if let Some(i) = self.sram_index(bank, off) {
            return self.sram[i];
        }
        let (b, o) = (bank as usize, off as usize);
        let len = self.rom.len();
        match self.mapping {
            Mapping::LoRom if o >= 0x8000 => self.rom[((b & 0x7F) * 0x8000 + (o - 0x8000)) % len],
            Mapping::HiRom if o >= 0x8000 || (0x40..=0x7D).contains(&(b & 0x7F)) => {
                self.rom[((b & 0x3F) * 0x10000 + o) % len]
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, bank: u8, off: u16, val: u8) {
        if let Some(i) = self.sram_index(bank, off) {
            self.sram[i] = val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lorom() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x7FC0..0x7FC0 + 21].fill(b' ');
        rom[0x7FC0..0x7FC4].copy_from_slice(b"TEST");
        rom[0x7FC0 + 0x15] = 0x20;
        rom[0x7FC0 + 0x1C..0x7FC0 + 0x1E].copy_from_slice(&0xFFFFu16.to_le_bytes());
        rom[0x7FFC] = 0x00;
        rom[0x7FFD] = 0x80;
        let c = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(c.mapping, Mapping::LoRom);
        assert_eq!(c.title, "TEST");
        assert_eq!(c.read(0, 0xFFFC), 0x00);
        assert_eq!(c.read(0, 0xFFFD), 0x80);
    }
}
