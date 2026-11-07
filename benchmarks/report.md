# Browser memory benchmark

## Test machine

- **Platform:** Colab
- **CPU:** Intel(R) Xeon(R) CPU @ 2.20GHz (2 cores)
- **RAM:** 12.7 GB
- **GPU:** none (software rendering, virtual screen)
- **OS:** Ubuntu 24.04.5 LTS, kernel 6.6.159+
- **Display:** Xvfb 1366x768x24
- **Date:** 2026-11-07 20:17
- **Method:** whole process tree, uss_mb, median of 3 runs, 20 s after launch

## Results by category (median over all pages, lower is better)

### Servo engine

| Browser | Version | Memory (MB) | vs pteron |
|---|---|---|---|
| servoshell | v0.7.0 | 258 | 0.83x |
| pteron | build 62 | 311 | 1.0x |

### Firefox (Gecko)

| Browser | Version | Memory (MB) | vs pteron |
|---|---|---|---|
| firefox | Mozilla Firefox 159.0.1 | 435 | 1.4x |

### Chromium

| Browser | Version | Memory (MB) | vs pteron |
|---|---|---|---|
| chrome | Google Chrome 157.0.8103.38 | 317 | 1.02x |

## Per page (MB)

| page | servoshell | pteron | firefox | chrome |
|---|---|---|---|---|
| blank | 169 | 212 | 420 | 274 |
| text | 373 | 417 | 433 | 319 |
| table | 219 | 266 | 427 | 318 |
| css | 258 | 311 | 436 | 294 |
| js | 485 | 531 | 560 | 484 |
| example | 229 | 279 | 417 | 284 |
| wikipedia | 364 | 413 | 504 | 354 |
| hackernews | 219 | 274 | 424 | 286 |
| github | 359 | 412 | 536 | 401 |

## Ten tabs open (MB; the last column is after the longest wait, when sleeping tabs have had time to sleep)

| Category | Browser | 10 tabs @1 min | 10 tabs @7 min | vs pteron (last) |
|---|---|---|---|---|
| Servo engine | pteron | 1298 | 946 | 1.00x |
| Servo engine | servoshell | 1241 | 1241 | 1.31x |
| Firefox (Gecko) | firefox | 888 | 788 | 0.83x |
| Chromium | chrome | 571 | 586 | 0.62x |

Single-page tests favour small engines; Servo renders some sites worse than Firefox and Chrome. Compare within this run only.