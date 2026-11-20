//! Картридж: определение схемы адресации и сопроцессора по заголовку, чтение ROM и SRAM.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mapping {
    LoRom,
    HiRom,
    /// HiROM больше 4 МБ (вторая половина ROM видна в банках $00–$3F и $40–$7D).
    ExHiRom,
}

/// Сопроцессор на картридже (по байту типа в заголовке).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Chip {
    None,
    Dsp,
    SuperFx,
    Obc1,
    Sa1,
    Sdd1,
    Rtc,
    Spc7110,
    Seta,
    Cx4,
    Other,
}

impl Chip {
    pub fn name(self) -> &'static str {
        match self {
            Chip::None => "нет",
            Chip::Dsp => "DSP-1/2/3/4",
            Chip::SuperFx => "Super FX",
            Chip::Obc1 => "OBC-1",
            Chip::Sa1 => "SA-1",
            Chip::Sdd1 => "S-DD1",
            Chip::Rtc => "S-RTC",
            Chip::Spc7110 => "SPC7110",
            Chip::Seta => "Seta ST010/011/018",
            Chip::Cx4 => "Cx4",
            Chip::Other => "неизвестный сопроцессор",
        }
    }
}

pub struct Cartridge {
    pub rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub mapping: Mapping,
    pub title: String,
    pub dirty: bool, // SRAM менялась с момента последнего сохранения
    pub chip: Chip,
    pub country: u8, // байт региона из заголовка ($FFD9)
}

/// Оценка правдоподобности заголовка по смещению `base`.
fn score(rom: &[u8], base: usize, kind: Mapping) -> i32 {
    if rom.len() < base + 0x40 {
        return -100;
    }
    let h = &rom[base..base + 0x40];
    let comp = u16::from_le_bytes([h[0x1C], h[0x1D]]);
    let sum = u16::from_le_bytes([h[0x1E], h[0x1F]]);
    let reset = u16::from_le_bytes([h[0x3C], h[0x3D]]);
    let mode = h[0x15];
    let mut s = 0;
    if comp ^ sum == 0xFFFF {
        s += 4;
    }
    match kind {
        Mapping::ExHiRom => {
            if mode & 0x0F == 5 {
                s += 3;
            }
        }
        Mapping::HiRom => {
            if mode & 1 == 1 {
                s += 2;
            }
        }
        Mapping::LoRom => {
            if mode & 1 == 0 {
                s += 2;
            }
        }
    }
    if reset >= 0x8000 {
        s += 2;
    }
    if h[..21].iter().all(|&c| (0x20..0x7F).contains(&c)) {
        s += 1;
    }
    s
}

/// Тип сопроцессора: старший полубайт байта $FFD6, если младший ≥ 3 («ROM + чип»).
fn detect_chip(rom: &[u8], base: usize) -> Chip {
    let ty = rom[base + 0x16];
    if ty & 0x0F < 3 {
        return Chip::None;
    }
    match ty >> 4 {
        0 => Chip::Dsp,
        1 => Chip::SuperFx,
        2 => Chip::Obc1,
        3 => Chip::Sa1,
        4 => Chip::Sdd1,
        5 => Chip::Rtc,
        0xF => match rom[base - 1] {
            // расширенный заголовок: подтип чипа
            0x00 => Chip::Spc7110,
            0x01 | 0x02 => Chip::Seta,
            0x10 => Chip::Cx4,
            _ => Chip::Other,
        },
        _ => Chip::Other,
    }
}

/// Зеркалирование адреса в ROM произвольного (не только степени двойки) размера:
/// так отображается, например, ROM на 3 МБ — верхняя часть повторяет последний мегабайт.
fn mirror(mut addr: usize, mut size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    let mut base = 0usize;
    let mut mask = 1usize << 23;
    while addr >= size {
        while addr & mask == 0 {
            mask >>= 1;
        }
        addr -= mask;
        if size > mask {
            size -= mask;
            base += mask;
        }
        mask >>= 1;
    }
    base + addr
}

impl Cartridge {
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        // Копир-заголовок 512 байт (.smc)
        let rom = if data.len() % 0x400 == 0x200 { &data[0x200..] } else { data };
        if rom.len() < 0x8000 {
            return Err("Файл слишком мал для SNES ROM".into());
        }
        let mut best = (i32::MIN, Mapping::LoRom, 0x7FC0usize);
        for (m, b) in [(Mapping::LoRom, 0x7FC0usize), (Mapping::HiRom, 0xFFC0), (Mapping::ExHiRom, 0x40FFC0)] {
            let s = score(rom, b, m);
            if s > best.0 {
                best = (s, m, b);
            }
        }
        let (_, mapping, base) = best;
        let h = &rom[base..base + 0x40];
        let title = String::from_utf8_lossy(&h[..21]).trim().to_string();
        let sram_kb = if h[0x18] == 0 || h[0x18] > 9 { 0 } else { 1usize << h[0x18] };
        Ok(Cartridge {
            rom: rom.to_vec(),
            sram: vec![0; sram_kb * 1024],
            mapping,
            title,
            dirty: false,
            chip: detect_chip(rom, base),
            country: h[0x19],
        })
    }

    pub fn chip_supported(&self) -> bool {
        self.chip == Chip::None
    }

    fn sram_index(&self, bank: u8, off: u16) -> Option<usize> {
        if self.sram.is_empty() {
            return None;
        }
        let (b, o) = (bank as usize & 0x7F, off as usize);
        let idx = match self.mapping {
            Mapping::LoRom if (0x70..=0x7D).contains(&b) && o < 0x8000 => (b - 0x70) * 0x8000 + o,
            Mapping::HiRom | Mapping::ExHiRom if (0x20..=0x3F).contains(&b) && (0x6000..0x8000).contains(&o) => {
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
        let addr = match self.mapping {
            Mapping::LoRom if o >= 0x8000 => (b & 0x7F) * 0x8000 + (o - 0x8000),
            Mapping::HiRom if o >= 0x8000 || (0x40..=0x7D).contains(&(b & 0x7F)) => (b & 0x3F) * 0x10000 + o,
            Mapping::ExHiRom => match b {
                0x00..=0x3F if o >= 0x8000 => 0x40_0000 + b * 0x10000 + o,
                0x80..=0xBF if o >= 0x8000 => (b - 0x80) * 0x10000 + o,
                0x40..=0x7D => 0x40_0000 + (b - 0x40) * 0x10000 + o,
                0xC0..=0xFF => (b - 0xC0) * 0x10000 + o,
                _ => return 0,
            },
            _ => return 0,
        };
        self.rom[mirror(addr, self.rom.len())]
    }

    pub fn write(&mut self, bank: u8, off: u16, val: u8) {
        if let Some(i) = self.sram_index(bank, off) {
            self.sram[i] = val;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ROM на 32 КБ с корректным заголовком LoROM.
    fn lorom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x7FC0..0x7FC0 + 21].fill(b' ');
        rom[0x7FC0..0x7FC4].copy_from_slice(b"TEST");
        rom[0x7FC0 + 0x15] = 0x20;
        rom[0x7FC0 + 0x1C..0x7FC0 + 0x1E].copy_from_slice(&0xFFFFu16.to_le_bytes());
        rom[0x7FFC] = 0x00;
        rom[0x7FFD] = 0x80;
        rom
    }

    #[test]
    fn detects_lorom() {
        let c = Cartridge::from_bytes(&lorom()).unwrap();
        assert_eq!(c.mapping, Mapping::LoRom);
        assert_eq!(c.title, "TEST");
        assert_eq!(c.read(0, 0xFFFC), 0x00);
        assert_eq!(c.read(0, 0xFFFD), 0x80);
        assert!(c.chip_supported());
    }

    #[test]
    fn detects_coprocessor_from_header() {
        let mut rom = lorom();
        rom[0x7FC0 + 0x16] = 0x13; // ROM + Super FX
        let c = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(c.chip, Chip::SuperFx);
        assert!(!c.chip_supported());
        rom[0x7FC0 + 0x16] = 0x02; // ROM + RAM + батарея: без сопроцессора
        assert_eq!(Cartridge::from_bytes(&rom).unwrap().chip, Chip::None);
    }

    #[test]
    fn mirrors_non_power_of_two_sizes() {
        assert_eq!(mirror(5, 8), 5);
        assert_eq!(mirror(0x9_0000, 0x8_0000), 0x1_0000);
        assert_eq!(mirror(0x35_0000, 0x30_0000), 0x25_0000); // 3 МБ: верх повторяет последний мегабайт
    }

    #[test]
    fn exhirom_maps_both_halves() {
        let mut rom = vec![0u8; 0x42_0000];
        let h = 0x40_FFC0;
        rom[h..h + 21].fill(b' ');
        rom[h..h + 2].copy_from_slice(b"EX");
        rom[h + 0x15] = 0x25;
        rom[h + 0x1C..h + 0x1E].copy_from_slice(&0xFFFFu16.to_le_bytes());
        rom[0x40_FFFC] = 0x00;
        rom[0x40_FFFD] = 0x80;
        rom[0x40_8000] = 0xAB; // верхняя половина: банк $00, смещение $8000
        rom[0x1234] = 0xCD; // нижняя половина: банк $C0, смещение $1234
        let c = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(c.mapping, Mapping::ExHiRom);
        assert_eq!(c.read(0x00, 0x8000), 0xAB);
        assert_eq!(c.read(0x40, 0x8000), 0xAB);
        assert_eq!(c.read(0xC0, 0x1234), 0xCD);
        assert_eq!(c.read(0x00, 0xFFFD), 0x80); // вектор сброса виден как у HiROM
    }
}
