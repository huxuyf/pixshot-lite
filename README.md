# PixShot Lite —— 截图 / 贴图

仿 Snipaste 的 Windows 桌面小工具：常驻托盘，一键截图，贴图钉在桌面。
纯 Rust 实现，单文件发布，无额外运行时依赖。

> **精简版**：只做截图与贴图，**不含 OCR**。需要 OCR 请用增强版
> [`huxuyf/pixshot`](https://github.com/huxuyf/pixshot)（同一基线 + OCR）。

## 功能

| 功能 | 说明 |
|------|------|
| **托盘常驻** | 启动后只驻留任务栏托盘，不弹窗。全局热键（默认 **F1**，可配置）或左键点击托盘图标开始截图；右键菜单含「开机启动」与「退出」 |
| **截图** | 热键后整屏冻结（不做暗化），十字光标拖动框选并叠加红色框线，Esc 取消。松手后冻结画面与红框保留，直到点工具栏按钮才一起消失。只做区域截图 |
| **工具栏** | 选区底部的紧凑白色圆角卡片，四个纯图标按钮：**贴图 / 保存 / 复制并关闭 / 关闭**，悬停出提示气泡（含快捷键）。点任一按钮后自动关闭 |
| **贴图** | 截图以无边框置顶小窗钉在桌面，四周带柔和投影。左键拖动、滚轮以光标为锚点缩放、右键菜单（复制 / 保存 / 重置缩放 / 关闭）、Esc 关闭 |
| **保存命名** | 默认文件名 `截图_YYYYMMDD_NN.<ext>`，按当天已有文件递增（01…99） |
| **提示** | 操作反馈用非模态 Toast（右下角自动消失），不弹系统消息框打断操作 |
| **配置** | 同目录 `config.ini` 纯文本配置：热键、保存格式、默认目录、Toast 时长 |

支持保存为 **PNG / JPEG / BMP**，或复制为位图到剪贴板。

## 构建与运行

**运行要求：Windows 7 SP1 及以上（x64）**，含 Windows 8 / 8.1 / 10 / 11。
Windows 7 / 8 上有若干能力降级，见「已知限制」。

**构建要求**比运行要求苛刻，因为要产出 Windows 7 兼容的二进制：

- Rust **nightly** 工具链，且需要 `rust-src`：

  ```powershell
  rustup toolchain install nightly-2026-10-27-x86_64-pc-windows-gnu --component rust-src
  ```

  发布目标 `x86_64-win7-windows-gnu` 是 tier-3 目标，rustup 不提供预编译 std，
  必须 `-Z build-std` 现场编译 std，所以只有 nightly 能做发布构建。
  刻意**不写** `rust-toolchain.toml`：rustup 的工具链文件不接受带宿主三元组的名字，
  实测在 GitHub runner 上会让仓库内的 `rustc -vV` 直接失败。
- **MSVCRT 版 MinGW-w64**（MSYS2 的 `mingw64` 环境，提供 `gcc` / `windres`）——
  GNU 宿主工具链的链接器，同时 `build.rs` 用它把图标与进程清单嵌进 exe。
  **别用 UCRT 版 MinGW**：那样链接出来的产物会导入 `api-ms-win-crt-*` 一族
  （其中 `api-ms-win-crt-private-l1-1-0.dll` 在 Windows 7 上并不存在），启动即失败。
  命令 `gcc -v` 若显示 `--with-default-msvcrt=ucrt`，就说明拿错了。

为什么必须专门走这个目标：官方默认目标（`x86_64-pc-windows-msvc` 等）编出来的产物会
静态依赖 Windows 8 / 10 才有的系统原语（`WaitOnAddress`、`ProcessPrng`、
`GetSystemTimePreciseAsFileTime`、`GetHostNameW`，以及 `GetDpiForWindow` 一族），
在 Windows 7 上**加载期**就失败（报「缺少 xxx.dll」或「无法定位程序输入点」）。
背景与取舍见 [docs/win7-support-plan.md](docs/win7-support-plan.md)。

```powershell
# 发布构建（Windows 7 兼容；首次约 2 分钟，主要时间花在现场编译 std）
# 工具链与日期必须显式写出，理由见上；CI 用的是同一个名字。
cargo +nightly-2026-10-27-x86_64-pc-windows-gnu build --release -Z build-std --target x86_64-win7-windows-gnu

# 日常开发（走本机默认目标，不付 build-std 的代价）
cargo run --release             # 运行（常驻托盘，按 F1 或点托盘图标截图）
cargo test                      # 单元 + 集成测试
pwsh -File scripts\ci.ps1       # 本地 CI（check → test → build → Win7 兼容性闸门 → 体积护栏 → E2E）
```

发布产物：`target\x86_64-win7-windows-gnu\release\pixshot.exe`（单文件，约 1.9 MB）。
比改造前略大，是因为 std 由 `-Z build-std` 现场编译；体积基线与 `ci.ps1` 的护栏已同步调整。

与增强版文件名相同，两版可在同一台机器上同时常驻；分发时建议按版本改名，
如 `pixshot-lite.exe` / `pixshot-plus.exe`。

### 高 DPI（截图清晰度）

截图按**物理分辨率**捕获，因此在 125% / 150% 等显示缩放下依然清晰
（例：125% 缩放的 1920×1080 屏，截出来就是 1920×1080，不是被放大过的 1536×864）。

这靠两道保障共同实现：

- `app.manifest` 在进程启动前声明 `PerMonitorV2`（主保障，不会静默失效）；
- `src/main.rs` 启动时再走一遍 DPI 感知设置作兜底，并校验实际生效的级别写入
  `pixshot_diag.log`（`[boot] DPI 感知级别 = 2（本平台可达上限 2）`）。

校验的**预期值按平台可达上限动态取**，而不是写死 2：Windows 7 / 8 没有每显示器 DPI，
系统感知（级别 1）就是那些系统的上限，此时日志不会误报。若在 Windows 8.1 / 10 / 11 上
看到「低于本平台可达上限」，说明两道保障都没生效，截图会被系统按逻辑分辨率捕获后
放大（即"模糊"），需要排查构建是否漏嵌了清单。

跳过资源嵌入（会**同时**跳过图标与清单，DPI 仅剩运行时兜底）：
`PIXSHOT_NO_ICON=1 cargo build`。

## 配置文件

首次启动会在 `pixshot.exe` 同目录生成带注释的 `config.ini`（UTF-8）。
**修改后重启生效**，删除该文件即恢复默认。

```ini
hotkey   = F1                ; 全局热键，支持 Ctrl / Alt / Shift / Win 组合，如 Ctrl+Shift+S
format   = png               ; 保存格式：png / jpg / bmp
save_dir =                   ; 默认保存目录，留空则每次从系统默认位置开始
toast_ms = 1600              ; 操作提示的停留毫秒数
```

非法值会自动回退默认，不会因配置写坏而启动失败。

## 操作方式

- **启动**：常驻托盘，不弹出窗口
- **触发截图**：全局热键（默认 **F1**）或左键点击托盘图标
- **框选**：拖动左键画出红色框线，松开后以框内区域为截图范围；单击或过小的拖动不产生选区，可继续框选；Esc 取消
- **工具栏**：贴图 / 保存图像 / 复制并关闭 / 关闭
- **工具栏快捷键**：**Ctrl+T** 贴图 / **Ctrl+S** 保存 / **Ctrl+C** 复制并关闭 / **Esc** 关闭
- **贴图窗口**：左键拖动移动，滚轮缩放，双击左键或 Esc 关闭；右键菜单含复制 / 保存 / 重置缩放 / 关闭
- **开机启动**：右键托盘 →「开机启动」切换，无需管理员权限
- **退出**：右键托盘 →「退出」

## 常见问题

程序每次启动会在 exe 同目录写诊断日志 `pixshot_diag.log`（不可写时回退 `%TEMP%`），
遇到问题先看它。一键采集：`pwsh -File scripts\diag_tray_hotkey.ps1`。

| 现象 | 解决 |
|---|---|
| 进程在运行但托盘没有图标 | Windows 11 默认把新图标收进「^」溢出区：点任务栏 `^` 找到图标并拖到托盘区固定。若由受限终端启动，改为在文件资源管理器里双击运行 |
| 按 F1 无反应 | F1 可能被其他程序占用，本版本会自动回退到键盘钩子兜底；也可在 `config.ini` 里换一个热键 |
| 双击运行也失败、托盘图标空白 | 运行目录被打了低完整性标签（常见于沙箱类工具写入的目录）。运行 `pwsh -File scripts\fix_integrity.ps1` 修复，或把 exe 复制到普通目录再运行 |
| 提示「PixShot 已启动，无需再次执行」 | 单实例保护：先在任务管理器结束所有 `pixshot.exe` 再启动 |
| 截图模糊、文字看不清 | 检查 `pixshot_diag.log` 里的 `[boot] DPI 感知级别`：Windows 8.1 / 10 / 11 上应为 `2（本平台可达上限 2）`；Windows 7 / 8 上应为 `1（本平台可达上限 1）`，属正常。若**低于上限**，说明进程未按该平台最高级别运行、截图被系统放大；优先确认所用的 exe 是正常构建产物（含 `app.manifest`），并检查 exe 属性 →「兼容性」→「更改高 DPI 设置」里是否误勾了「替代高 DPI 缩放行为」 |

## 已知限制

- 仅支持 Windows（Windows 7 SP1 及以上，x64）
- **Windows 7 / 8 上的能力降级**（系统本身缺对应 API，程序自动回退）：
  - DPI 感知只能到**系统级**；每显示器感知需要 Windows 8.1 起步，
    多显示器缩放比例不同时截图与贴图坐标可能有偏差；
  - 没有 Mica / 圆角 / 深色标题栏（Win10/11 的 DWM 能力），界面为实色表面；
  - 窗口「排除出截屏捕获」需要 Windows 10 2004 起步，更早系统上自动跳过。
- 本版不含 OCR；需要识别文字请用增强版 [`huxuyf/pixshot`](https://github.com/huxuyf/pixshot)
- 两版默认热键都是 F1，同时运行会互相抢占，需要并用请给其中一个改 `config.ini`
- 多显示器基于虚拟屏幕坐标，支持负坐标副屏
- 贴图只支持滚轮缩放，不提供边缘拖拽调整
- 配置修改后需重启生效
- 发布的 exe 未签名，分发到其他机器会遇到 SmartScreen 警告（「仍要运行」可放行）
