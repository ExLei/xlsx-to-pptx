# Windows 交叉构建（WSL → x86_64-pc-windows-msvc）

正式发布以 GitHub Actions 的 windows-latest 原生构建为准（`release.yml`）。
本页记录从 WSL 交叉产出 Windows exe 的正确姿势，供本地验证与应急发布使用。

## 步骤

1. 工具：`cargo-xwin`（`~/.cargo/bin`）、`clang-cl` + `lld-link`（llvm/lld 包）；
   xwin 首次会下载 MSVC CRT/SDK（网络不通时设 `HTTPS_PROXY`）。
2. 先跑一次构建以创建 OUT_DIR（会因缺少着色器字节而失败，属预期）：
   `cargo xwin build --release --target x86_64-pc-windows-msvc -p x2p-app`
3. 生成并注入着色器字节：`scripts/win-cross-shaders.sh`
4. 再构建：`cargo xwin build --release --target x86_64-pc-windows-msvc -p x2p-app`
   → 产物 `target/x86_64-pc-windows-msvc/release/英语助记卡片生成.exe`

## 两个必须知道的坑

1. **build.rs 只在 Windows 宿主生成着色器**：`gpui-pre-windows` 的 build.rs 以
   `#[cfg(target_os = "windows")]` 判断宿主；交叉构建时它不会产出
   `OUT_DIR/shaders_bytes.rs`，而 release 的渲染器代码 `include!` 该文件。
2. **Wine 会偷偷换掉编译器**：`fxc.exe` 经 Wine 调用时默认加载 Wine **内置** 的
   `d3dcompiler_47`（实现为 vkd3d-shader）——产物与微软原生 fxc 不一致，
   真机表现为「界面几何与图标正常、**全部文字不渲染**」。
   必须 `WINEDLLOVERRIDES=d3dcompiler_47=n` 强制使用 Windows SDK 里那份原生 dll
   （`scripts/win-cross-shaders.sh` 已处理）。

## 验证方法

- 产物 exe 内应含 `Microsoft (R) HLSL Shader Compiler 10.1` 标记，且**不含** `vkd3d-shader`。
- 与原生 Windows 构建的 `target/release/build/gpui-pre-windows-*/out/shaders_bytes.rs`
  可逐字节比对（18 个常量应完全一致）。

## 备注

- Wine 跑不了本 GUI（其 DWrite 枚举不到系统字体，gpui 启动即 panic）；
  交叉产物的 GUI 行为必须在真机验证，CLI 路径可用 Wine 冒烟。
- 诊断开关：`X2P_TEXT_MODE=gray|subpixel` 可覆盖文本渲染模式（默认平台默认，不受影响）。
