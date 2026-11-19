// Приёмник звука: берёт стерео-отсчёты DSP (32 кГц) из очереди и ресемплирует
// их в частоту AudioContext линейной интерполяцией.
class SnesAudio extends AudioWorkletProcessor {
  constructor() {
    super();
    this.q = [];       // чанки Int16Array (L, R вперемешку)
    this.qi = 0;       // позиция (в парах) в первом чанке
    this.pairs = 0;    // пар в очереди
    this.step = 32000 / sampleRate;
    this.frac = 0;
    this.al = 0; this.ar = 0; this.bl = 0; this.br = 0;
    this.pl = 0; this.pr = 0;
    this.port.onmessage = (e) => {
      this.q.push(e.data);
      this.pairs += e.data.length >> 1;
      // не копим задержку больше ~120 мс
      while (this.pairs > 3840 && this.q.length > 1) {
        const d = this.q.shift();
        this.pairs -= (d.length >> 1) - this.qi;
        this.qi = 0;
      }
    };
  }

  next() {
    while (this.q.length) {
      const d = this.q[0];
      if (this.qi * 2 < d.length) {
        this.pl = d[this.qi * 2];
        this.pr = d[this.qi * 2 + 1];
        this.qi++;
        this.pairs--;
        return true;
      }
      this.q.shift();
      this.qi = 0;
    }
    return false;
  }

  process(inputs, outputs) {
    // динамическая подстройка скорости (±0.5%): держит очередь около 50 мс при любой частоте кадров
    const err = (this.pairs - 1600) / 1600;
    const step = this.step * (1 + Math.max(-0.005, Math.min(0.005, err * 0.01)));
    const L = outputs[0][0];
    const R = outputs[0][1] || L;
    for (let i = 0; i < L.length; i++) {
      this.frac += step;
      while (this.frac >= 1) {
        this.frac -= 1;
        this.al = this.bl; this.ar = this.br;
        if (this.next()) { this.bl = this.pl; this.br = this.pr; } else { this.bl = 0; this.br = 0; }
      }
      L[i] = (this.al + (this.bl - this.al) * this.frac) / 32768;
      R[i] = (this.ar + (this.br - this.ar) * this.frac) / 32768;
    }
    return true;
  }
}
registerProcessor('snes-audio', SnesAudio);
