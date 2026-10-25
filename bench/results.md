| case | exit | lines | raw | `tail -15` | `tail -50` | q | raw / q | failure in `tail -15` / `tail -50` / q |
|---|---|---|---|---|---|---|---|---|
| rg-test | 0 | 1404 | 16,001 | 216 | 577 | 264 | 60.6x | - |
| express-test-fail | 1 | 1994 | 15,160 | 234 | 461 | 316 | 48.0x | yes / yes / yes |
| express-test | 0 | 1972 | 14,903 | 104 | 323 | 143 | 104.2x | - |
| hono-test | 0 | 154 | 3,994 | 286 | 1,302 | 333 | 12.0x | - |
| rg-test-fail | 101 | 323 | 3,562 | 147 | 484 | 261 | 13.6x | yes / yes / yes |
| flask-test-fail | 1 | 132 | 1,304 | 190 | 486 | 510 | 2.6x | yes / yes / yes |
| hono-test-fail | 1 | 82 | 1,109 | 228 | 698 | 407 | 2.7x | **no** / **no** / yes |
| rg-compile-error | 101 | 41 | 549 | 210 | 549 | 586 | 0.9x | yes / yes / yes |
| rg-build-clean | 0 | 35 | 538 | 288 | 537 | 574 | 0.9x | - |
| flask-test | 0 | 35 | 388 | 179 | 388 | 422 | 0.9x | - |
| flask-import-error | 4 | 13 | 137 | 136 | 136 | 172 | 0.8x | yes / yes / yes |
| hono-type-error | 1 | 1 | 29 | 29 | 29 | 67 | 0.4x | yes / yes / yes |
| express-lint | 0 | 4 | 14 | 14 | 14 | 48 | 0.3x | - |
| **total** | | | 57,688 | 2,261 | 5,984 | 4,103 | 14.1x | 6/7 / 6/7 / 7/7 |
