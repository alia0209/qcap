#!/usr/bin/env bash
# 300 lines of output: a noisy build with one error at line 150 and a refusal at the end.
for i in $(seq 1 299); do
  if ((i == 150)); then echo "error: something broke" >&2; else echo "line $i ok"; fi
done
echo "REFUSED lost_edit (exit 3)"
exit 3
