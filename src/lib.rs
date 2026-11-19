mod apu;
mod bus;
mod cartridge;
mod cpu;
mod dsp;
mod dma;
mod ppu;
#[cfg(test)]
mod rom_tests;

use wasm_bindgen::prelude::*;

const LINES: u16 = 262; // NTSC
const LINE_CLOCKS: u32 = 1364; // master-тактов на строку
const VBLANK_START: u16 = 225;

#[wasm_bindgen]
pub struct Emulator {
    cpu: cpu::Cpu,
    bus: bus::Bus,
    frame: u32,
    carry: u32, // такты, выходящие за конец прошлой строки
}

impl Emulator {
    fn start_line(&mut self, v: u16) {
        let b = &mut self.bus;
        b.vline = v;
        if v == 0 {
            b.vblank = false;
            b.nmi_flag = false;
            b.hdma_init();
        }
        if v == VBLANK_START {
            b.vblank = true;
            b.nmi_flag = true;
            if b.nmitimen & 0x80 != 0 { b.nmi_trigger = true; }
            if b.nmitimen & 1 != 0 {
                b.joy1_latched = b.joy1; // автоопрос джойпада
                b.joy_busy = true;
            }
        }
        if v == VBLANK_START + 3 { b.joy_busy = false; }
        if v < VBLANK_START { b.hdma_run(); }
        // строка 1..=224 рисуется после HDMA, когда регистры уже обновлены
        if (1..=224).contains(&v) { b.ppu.render_line((v - 1) as usize); }
        // IRQ только по V: срабатывает в начале строки VTIME
        if (b.nmitimen >> 4) & 3 == 2 && v == b.vtime { b.timeup = true; }
    }

    /// IRQ по H (и по H+V): срабатывает, если момент HTIME попал в [h, next).
    fn check_irq(&mut self, v: u16, h: u32, next: u32) {
        let t = self.bus.htime as u32 * 4;
        let armed = match (self.bus.nmitimen >> 4) & 3 {
            1 => true,
            3 => v == self.bus.vtime,
            _ => false,
        };
        if armed && h <= t && t < next { self.bus.timeup = true; }
    }
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new(rom: &[u8]) -> Result<Emulator, JsValue> {
        let cart = cartridge::Cartridge::from_bytes(rom).map_err(|e| JsValue::from_str(&e))?;
        let mut bus = bus::Bus::new(cart);
        let mut cpu = cpu::Cpu::default();
        cpu.reset(&mut bus);
        Ok(Emulator { cpu, bus, frame: 0, carry: 0 })
    }

    pub fn title(&self) -> String {
        self.bus.cart.title.clone()
    }

    pub fn mapping(&self) -> String {
        format!("{:?}", self.bus.cart.mapping)
    }

    /// Маска кнопок в формате регистров JOY1 ($4218/$4219):
    /// бит15 B, 14 Y, 13 Select, 12 Start, 11 Up, 10 Down, 9 Left, 8 Right,
    /// бит7 A, 6 X, 5 L, 4 R.
    pub fn set_joypad(&mut self, mask: u16) {
        self.bus.joy1 = mask;
    }

    /// Один кадр: 262 строки по 1364 master-тактов.
    pub fn run_frame(&mut self) {
        for v in 0..LINES {
            self.start_line(v);
            let mut h = self.carry;
            while h < LINE_CLOCKS {
                if self.bus.nmi_trigger {
                    self.bus.nmi_trigger = false;
                    self.cpu.nmi = true;
                }
                self.cpu.irq = self.bus.timeup;
                self.bus.hclock = h;
                let cycles = self.cpu.step(&mut self.bus) + std::mem::take(&mut self.bus.stall);
                self.bus.apu.clock(cycles);
                let next = h + cycles;
                self.check_irq(v, h, next);
                h = next;
            }
            self.carry = h - LINE_CLOCKS;
        }
        self.frame = self.frame.wrapping_add(1);
    }

    pub fn cpu_state(&self) -> String {
        self.cpu.debug()
    }

    /// Накопленные стерео-отсчёты DSP (32 кГц, L/R вперемешку).
    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.bus.apu.out)
    }

    pub fn sram_len(&self) -> usize {
        self.bus.cart.sram.len()
    }

    pub fn sram(&self) -> Vec<u8> {
        self.bus.cart.sram.clone()
    }

    pub fn load_sram(&mut self, data: &[u8]) {
        let n = data.len().min(self.bus.cart.sram.len());
        self.bus.cart.sram[..n].copy_from_slice(&data[..n]);
    }

    /// true, если SRAM менялась с прошлого вызова.
    pub fn take_sram_dirty(&mut self) -> bool {
        std::mem::take(&mut self.bus.cart.dirty)
    }

    pub fn spc_state(&self) -> String {
        self.bus.apu.debug()
    }

    /// Указатель на RGBA-буфер 256x224 в памяти wasm.
    pub fn framebuffer_ptr(&self) -> *const u8 {
        self.bus.ppu.fb.as_ptr()
    }
}
