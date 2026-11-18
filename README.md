# snes-web

SNES-эмулятор на Rust → WebAssembly.

Локально:

    wasm-pack build --release --target web --out-dir web/pkg
    python3 -m http.server -d web 8080

Деплой: push в `main` → GitHub Actions → GitHub Pages
(Settings → Pages → Source: **GitHub Actions**).
