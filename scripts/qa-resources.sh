#!/usr/bin/env sh
# 参照資源のロック（qa-v0 段階 1）。specification/resources.lock に載せた資源を kpsewhich で解決し、sha256 を照合する。
# 使い方: scripts/qa-resources.sh check    CI で用いる。必須資源の不在、または digest の相違で 1 で終わる。
#         scripts/qa-resources.sh record   手元の TeX Live で lock の digest を書き直す。差分は変更管理（系列の ci-and-change.md）で見る。
# lock の行: 名前 <TAB> required|optional <TAB> sha256。'#' で始まる行は注記。
# 任意資源の不在は SABI_STRICT_OPTIONAL を設定したときだけ失敗にする。digest の相違は必須・任意を問わず失敗
# （別の版のファイルで PASS しても、基準に対する PASS ではない）。参照ツールの版は照合せず表示だけする。
set -eu
cd "$(dirname "$0")/.."
TOOLS="tex etex uptex xetex dvipdfmx kpsewhich"
lock=specification/resources.lock
mode=${1:-check}
status=0
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
echo "# tools"
for t in $TOOLS; do
  printf '%s: ' "$t"
  if command -v "$t" >/dev/null 2>&1; then "$t" --version 2>/dev/null | head -n 1; else echo "(missing)"; fi
done
echo "# resources"
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    ''|'#'*)
      if [ "$mode" = record ]; then printf '%s\n' "$line" >>"$tmp"; fi
      continue
      ;;
  esac
  name=$(printf '%s' "$line" | cut -f1)
  need=$(printf '%s' "$line" | cut -f2)
  want=$(printf '%s' "$line" | cut -f3)
  path=$(kpsewhich "$name" 2>/dev/null | tr -d '\r' || true)
  if [ -z "$path" ]; then
    have="-"
    st=ABSENT
  else
    have=$(sha256sum "$path" | cut -d' ' -f1)
    if [ "$mode" = record ] || [ "$have" = "$want" ]; then st=OK; else st=DIFFERENT; fi
  fi
  if [ "$mode" = record ]; then printf '%s\t%s\t%s\n' "$name" "$need" "$have" >>"$tmp"; fi
  printf '%-28s %-9s %-10s %s\n' "$name" "$need" "$st" "$path"
  case "$st" in
    DIFFERENT) status=1 ;;
    ABSENT) if [ "$need" = required ] || [ -n "${SABI_STRICT_OPTIONAL:-}" ]; then status=1; fi ;;
  esac
done <"$lock"
if [ "$mode" = record ]; then cp "$tmp" "$lock"; fi
exit $status
