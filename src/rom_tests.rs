//! Прогон внешних тестовых ROM (gilyon/snes-tests, лицензия MIT) без браузера.
//! Результат читается «с экрана»: ROM выводит текст в тайлкарту BG1 через PPU.
use super::Emulator;

/// Описание теста `num` (hex) из автогенерированного cputest-*.txt.
fn describe(descr: &str, num: &str) -> String {
    let head = format!("Test {}:", num.to_lowercase());
    let mut out = String::new();
    let mut inside = false;
    for line in descr.lines() {
        if line.starts_with("Test ") {
            inside = line.to_lowercase().starts_with(&head);
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.is_empty() { format!("(описание теста {} не найдено)", num) } else { out }
}

fn run_cputest(rom: &[u8], descr: &str) {
    let mut emu = Emulator::new(rom).ok().expect("ROM не загрузился");
    for _ in 0..3000 {
        emu.run_frame();
        let status = emu.bus.ppu.vram_text(0x32, 18);
        if status.starts_with("Success") {
            return;
        }
        if status.starts_with("Failed") || status.starts_with("Invalid") {
            let t = |w, n| emu.bus.ppu.vram_text(w, n);
            let num = t(0x6E, 4);
            panic!(
                "{} на тесте {}\nполучено: A={} X={} Y={} P={} S={}\n{}",
                status.trim_end_matches('\0'),
                num,
                t(0xA5, 4),
                t(0xC5, 4),
                t(0xE5, 4),
                t(0x105, 2),
                t(0x125, 4),
                describe(descr, &num)
            );
        }
    }
    let num = emu.bus.ppu.vram_text(0x6E, 4);
    panic!("тайм-аут: последний тест {}\nCPU: {}", num, emu.cpu_state());
}

#[test]
fn cputest_basic() {
    run_cputest(
        include_bytes!("../tests/data/cputest-basic.sfc"),
        include_str!("../tests/data/cputest-basic.txt"),
    );
}

#[test]
#[ignore = "полный набор (в т.ч. wrap в режиме эмуляции): cargo test cputest_full -- --ignored"]
fn cputest_full() {
    run_cputest(
        include_bytes!("../tests/data/cputest-full.sfc"),
        include_str!("../tests/data/cputest-full.txt"),
    );
}
