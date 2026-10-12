#!/usr/bin/env bash
# 为 Windows 交叉构建（cargo-xwin / x86_64-pc-windows-msvc）生成 gpui-pre-windows 的着色器字节。
#
# 为什么需要它：
#   1) gpui-pre-windows 的 build.rs 只在 Windows 宿主上生成 shaders_bytes.rs；
#      交叉构建必须先手工生成并注入 OUT_DIR，否则编译报缺少该文件。
#   2) fxc.exe 经 Wine 调用时默认加载 Wine 自带的内置 d3dcompiler_47（vkd3d-shader），
#      产物与微软原生 fxc 不一致，真机上会表现为「界面几何/图标正常、文字全部不渲染」。
#      必须用 WINEDLLOVERRIDES=d3dcompiler_47=n 强制使用 Windows SDK 的原生 dll。
#
# 用法：
#   scripts/win-cross-shaders.sh [FXC_EXE]
#   cargo xwin build --release --target x86_64-pc-windows-msvc -p x2p-app
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FXC="${1:-$(ls -1 "/mnt/c/Program Files (x86)/Windows Kits/10/bin/"*/x64/fxc.exe 2>/dev/null | sort -V | tail -1)}"
[ -n "$FXC" ] && [ -f "$FXC" ] || { echo "找不到 fxc.exe（可用参数指定）"; exit 1; }

CRATE_SRC="$(ls -d "$HOME"/.cargo/registry/src/*/gpui-pre-windows-*/src 2>/dev/null | sort -V | tail -1)"
[ -n "$CRATE_SRC" ] || { echo "找不到 gpui-pre-windows 源码（先 cargo fetch）"; exit 1; }
echo "fxc:   $FXC"
echo "crate: $CRATE_SRC"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
cp "$CRATE_SRC"/shaders.hlsl "$CRATE_SRC"/alpha_correction.hlsl "$CRATE_SRC"/color_text_raster.hlsl "$WORK/"

OUT_DIRS=$(ls -d "$ROOT"/target/x86_64-pc-windows-msvc/*/build/gpui-pre-windows-*/out 2>/dev/null || true)
[ -n "$OUT_DIRS" ] || { echo "先跑一次 cargo xwin build --release --target x86_64-pc-windows-msvc -p x2p-app 以创建 OUT_DIR"; exit 1; }

gen() { # entry target varname header hlsl
  WINEDEBUG=-all WINEDLLOVERRIDES="d3dcompiler_47=n;mscoree,mshtml=" \
    wine "$FXC" /T "$2" /E "$1" /Fh "Z:$WORK/$4" /Vn "$3" /O3 "$5" >/dev/null 2>&1
  [ -f "$WORK/$4" ] || { echo "编译失败: $1"; exit 1; }
}

MODULES="quad shadow path_rasterization path_sprite underline monochrome_sprite subpixel_sprite polychrome_sprite"
for m in $MODULES; do
  U=$(echo "$m" | tr 'a-z' 'A-Z')
  gen "${m}_vertex"   vs_4_1 "${U}_VERTEX_BYTES"   "${m}_vs.h" shaders.hlsl
  gen "${m}_fragment" ps_4_1 "${U}_FRAGMENT_BYTES" "${m}_ps.h" shaders.hlsl
done
gen "emoji_rasterization_vertex"   vs_4_1 "EMOJI_RASTERIZATION_VERTEX_BYTES"   "emoji_rasterization_vs.h" color_text_raster.hlsl
gen "emoji_rasterization_fragment" ps_4_1 "EMOJI_RASTERIZATION_FRAGMENT_BYTES" "emoji_rasterization_ps.h" color_text_raster.hlsl

BIND="$WORK/shaders_bytes.rs"; : > "$BIND"
emit() { # varname header
  python3 - "$1" "$2" "$BIND" <<'PY'
import sys
name, head, out = sys.argv[1], sys.argv[2], sys.argv[3]
c = open(head, errors="replace").read()
gv = c[c.find("const BYTE"):]
defn = gv[gv.find("=") + 1:].strip()
open(out, "a").write(f"const {name}: &[u8] = &{defn.replace('{','[').replace('}',']')}\n")
PY
}
for m in $MODULES; do
  U=$(echo "$m" | tr 'a-z' 'A-Z'); emit "${U}_VERTEX_BYTES" "$WORK/${m}_vs.h"; emit "${U}_FRAGMENT_BYTES" "$WORK/${m}_ps.h"
done
emit "EMOJI_RASTERIZATION_VERTEX_BYTES" "$WORK/emoji_rasterization_vs.h"
emit "EMOJI_RASTERIZATION_FRAGMENT_BYTES" "$WORK/emoji_rasterization_ps.h"

for d in $OUT_DIRS; do cp "$BIND" "$d/shaders_bytes.rs"; echo "写入: $d/shaders_bytes.rs"; done

# 自检：必须是微软编译器产物（不是 wine 内置 vkd3d）
python3 - "$BIND" <<'PY'
import sys
data = open(sys.argv[1], "rb").read()
assert b"Microsoft (R) HLSL Shader Compiler" in data, "产物不是微软 fxc（检查 WINEDLLOVERRIDES）"
assert b"vkd3d-shader" not in data, "产物来自 Wine 内置 d3dcompiler（vkd3d）"
print("自检通过：微软 HLSL 编译器产物，无 vkd3d 痕迹")
PY
echo "完成。接着运行: cargo xwin build --release --target x86_64-pc-windows-msvc -p x2p-app"
