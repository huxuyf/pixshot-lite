# PixShot Lite 支持 Windows 7 及以上版本的可行性方案

> 编写日期：2026-10-29
> 结论依据：本机实测（导入表 dump + toolchain 探针编译），非推断。文中每一项「实测」都可在文末
> 「附录 A：复现步骤」中重跑。

---

## 1. 可行性结论

**可行，但不是「打补丁」级别的工作量，而是需要改动构建工具链 + 依赖特性 + 约 10 处源码调用点。**

三条关键结论：

1. **当前产物在 Windows 7 上不可能启动**，而且卡点不止一个。缺的 DLL 有 4 个，缺的「入口点」
   还有 5 个。把报错的 `api-ms-win-core-winrt-error-l1-1-0.dll` 补上，只会继续撞下一个。
2. **常规的「往 exe 旁边丢垫片 DLL」路线走不通**：`kernel32.dll` 与 `ws2_32.dll` 都在系统的
   KnownDLLs 列表里（本机注册表 `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\KnownDLLs`
   已确认），无法被应用目录下的同名文件顶替，而 `GetSystemTimePreciseAsFileTime`、
   `GetHostNameW` 恰好来自这两个 DLL；`user32` 还缺 5 个 Win10 才有的入口点，代理 user32
   需要转发它全部导出，不现实。
3. **正解是 Rust 官方的 `win7` 目标（tier-3）+ `build-std`**。Rust 标准库本身有完整的
   `target_vendor = "win7"` 分支（线程停靠改用 keyed event、随机数改用 `RtlGenRandom`、
   时间改用 `GetSystemTimeAsFileTime`、hostname 走 unsupported 实现），只是**没有分发预编译
   std**，必须用 nightly 现场编译 std。我已经用一个探针工程实测跑通，产出的导入表**完全干净**。

工作量估计：源码改动集中在一个新模块 + 10 个调用点，加上构建/CI/文档调整。**主要成本不在写代码，
而在于引入 nightly 工具链与 `build-std`（可复现性与构建时长），以及需要一台 Win7 机器做验收。**

---

## 2. 关键兼容性限制（实测清单）

### 2.1 当前产物在 NT 6.1 上缺失的静态导入

`objdump -p target/release/pixshot.exe` 的实测结果：

| 缺失项 | 类型 | 来源 | 归属 |
|---|---|---|---|
| `api-ms-win-core-winrt-error-l1-1-0.dll` → `RoOriginateErrorW` | 整个 DLL 不存在 | WinRT（Win8+） | `windows-result 0.2.0`（`windows 0.58` 的传递依赖） |
| `api-ms-win-core-synch-l1-2-0.dll` → `WaitOnAddress` / `WakeByAddressAll` / `WakeByAddressSingle` | 整个 DLL 不存在 | Win8+ | **Rust 标准库** |
| `api-ms-win-shcore-scaling-l1-1-1.dll` → `GetDpiForMonitor` / `SetProcessDpiAwareness` | 整个 DLL 不存在 | Win8.1+ | `windows` crate 的 `Win32_UI_HiDpi` 特性 |
| `bcryptprimitives.dll` → `ProcessPrng` | 整个 DLL 不存在 | Win8+ | **Rust 标准库**（随机数播种） |
| `user32.dll` → `GetDpiForWindow`、`GetDpiForSystem`、`GetThreadDpiAwarenessContext`、`GetAwarenessFromDpiAwarenessContext` | DLL 在、函数不在 | Win10 1607+ | `Win32_UI_HiDpi` 特性 |
| `user32.dll` → `SetProcessDpiAwarenessContext` | DLL 在、函数不在 | Win10 1703+ | `Win32_UI_HiDpi` 特性 |
| `kernel32.dll` → `GetSystemTimePreciseAsFileTime` | DLL 在、函数不在 | Win8+ | **Rust 标准库**（`Instant` / `SystemTime`） |
| `ws2_32.dll` → `GetHostNameW` | DLL 在、函数不在 | Win8+ | **Rust 标准库**（net 模块所在 codegen unit） |

**两个性质要分清**：

- 「DLL 不存在」→ 报「缺少 xxx.dll」；
- 「函数不存在」→ 报「无法定位程序输入点 xxx 于 xxx.dll」。

用户看到的 `api-ms-win-core-winrt-error-l1-1-0.dll` 只是**加载器按导入表顺序撞到的第一个**。
这也解释了为什么单点修复没有意义。

### 2.2 标准库侧的障碍无法靠应用层规避

我写了一个**不含任何应用代码、不含 `windows` crate** 的纯 std 探针（用到线程、`Mutex`/`Condvar`/
`RwLock`/`OnceLock`、`HashMap`、`Instant`、`SystemTime`、`ToSocketAddrs`），用现有默认目标
`x86_64-pc-windows-gnu` 编译，实测复现了上表中**全部 4 项 std 侧问题**
（`api-ms-win-core-synch-l1-2-0`、`bcryptprimitives`、`GetSystemTimePreciseAsFileTime`、
`GetHostNameW`）。

**结论：只要用官方预编译 std，这些依赖就必然存在，应用层无从下手。**而 `kernel32`/`ws2_32`
属于 KnownDLL，垫片 DLL 也救不了（见第 3 章路线 D）。所以 `build-std` 是**必需**的，不是可选优化。

### 2.3 已经没问题的部分（勿做无用功）

- **应用清单 `app.manifest` 无需改动 DPI 声明**。查 Microsoft 文档确认：`<dpiAware>` 的
  `true/PM` 在 **Vista / Win7 / Win8 上按「系统 DPI 感知」处理**，在 Win8.1/Win10 上才是每显示器感知。
  现有写法 `<dpiAware>true/PM</dpiAware>` + `<dpiAwareness>PerMonitorV2</dpiAwareness>`
  在 Win7 上会正确降级为系统感知。**只有 `supportedOS` 需要补 GUID**（当前只声明了 Win10）。
- **图像编解码无需改动**。`imageio.rs` 通过 `CoCreateInstance(CLSID_WICImagingFactory)` 使用 WIC，
  走 COM 激活而非静态导入（所以导入表里看不到 `windowscodecs.dll`），WIC 自 Vista 起就在系统里。
  GDI+（`gdiplus.dll`）也在 KnownDLLs 中且 Win7 自带 1.1 版，本项目只用基础绘图 API。
- **DWM 相关调用天然降级**。`theme.rs` / `toast.rs` 里所有 `DwmSetWindowAttribute`
  都是 `let _ =` 忽略返回值的写法，Mica（`DWMWA_SYSTEMBACKDROP_TYPE`）、圆角
  （`DWMWA_WINDOW_CORNER_PREFERENCE`）、深色标题栏在 Win7 上会失败但不会崩。工具栏/贴图/Toast
  都是分层窗口自绘，不依赖 DWM 合成。
- **其余导入全部 Win7 可用**：`dwmapi`、`gdi32`、`gdiplus`、`imm32`、`comdlg32`、`propsys`、
  `ole32`、`oleaut32`、`shell32`、`userenv`、`advapi32`、`ntdll`。

### 2.4 必须接受的功能降级

| 能力 | Win10 1809+ | Win7 SP1 | 说明 |
|---|---|---|---|
| DPI 感知 | PerMonitorV2 | **仅系统级** | Win7 没有 API Set 层面的每显示器 DPI，`SetProcessDpiAwarenessContext` 等均不存在 |
| 多显示器不同缩放 | 正确 | **坐标会偏** | 系统只有一个 DPI 值，混合缩放场景下截图/贴图位置与尺寸不准确 |
| Mica / 圆角 / 深色标题栏 | 有 | 无 | 仅影响观感，不影响功能 |
| 窗口防捕获 | `WDA_EXCLUDEFROMCAPTURE`（完全排除） | 只有 `WDA_MONITOR`（捕获时变黑块） | **需要产品决策**，见第 8 章 |

---

## 3. 可选技术路线及取舍

### 路线 D：应用目录放垫片 DLL（**不可行，先排除**）

思路是不改代码，在 exe 旁放 4 个同名 stub DLL。实测与查证后否决：

- `kernel32.dll`、`ws2_32.dll`、`shcore.dll`、`user32.dll`、`gdiplus.dll` 等均在 KnownDLLs 列表，
  系统直接按名称绑定 System32 下的模块，应用目录的同名文件**不会被加载**；
- 因此 `GetSystemTimePreciseAsFileTime`（kernel32）与 `GetHostNameW`（ws2_32）**无法通过垫片提供**；
- `user32.dll` 缺 5 个入口点，要垫就得分发一个代理整个 user32 导出面的 DLL；
- `WaitOnAddress`/`WakeByAddress*` 被 std 的锁与 `Once` 使用，stub 成空实现会死锁或数据竞争，
  必须实现正确的 keyed-event 语义。

→ 唯一「看起来省事」的路线其实是坑最多的。

### 路线 C：降级到旧 Rust / 旧 `windows` crate（**不推荐**）

理论上用 Rust ≤ 1.77 + `windows 0.4x` 可以让 std 少依赖 Win8 原语。但：

- Rust 1.78 起官方最低支持上移到 Win10，1.77 的 std 是否完全不引入 Win8 原语**没有保证**，
  且需要逐版本实测；
- `windows 0.4x` 与 0.58 的 API 差异大（`windows::core::Result`、`link!` 宏、特性命名都改过），
  重构面比路线 A/B 大得多；
- 放弃新编译器的优化与安全修复，长期维护成本最高。

→ 只在前两条路线都不成立时才考虑。

### 路线 A：双产物（保留现有 + 新增 Win7 产物）

保留现有 `x86_64-pc-windows-msvc` 稳定工具链产物（Win10 1809+，PerMonitorV2 完整能力），
另外新增一个 Win7 兼容产物。

- 优点：现有用户与 CI 完全不受影响；Win10+ 上不牺牲任何能力；nightly/`build-std` 的风险被隔离在
  一条独立构建链里。
- 缺点：两份构建配置、两份测试矩阵、两份体积护栏；「同一版本号一个产物」的发布模型被打散
  （现在是 `Cargo.toml` 一个 version 对应一个 tag + 一个 exe）。

### 路线 B：单产物通吃（Win7 SP1 ~ Win11）

只出一份 win7 目标产物，全平台共用。

- 优点：一个 exe 覆盖 Win7 SP1 及以上，发布模型不变，验证一次即可。
- 缺点：**Win10/11 用户会丢掉 PerMonitorV2**，因为在 Win7 上 `SetProcessDpiAwarenessContext`
  不可用，能力探测会一路回退到系统级感知 —— 这是产品能力的**净退化**，不是等价替换。
  （注：清单里的 `PerMonitorV2` 仍会生效，但 `main.rs` 的运行时代码与 `verify_dpi_awareness`
  需要按平台调整预期，否则会持续告警。）

> ⚠️ 这一点需要澄清：因为 `app.manifest` 里的 `<dpiAwareness>PerMonitorV2</dpiAwareness>` 在
> Win10 1703+ 上仍然生效，路线 B **未必**真的退化到系统级感知；但 `set_dpi_awareness()` 的运行时
> 兜底链和 `verify_dpi_awareness()` 的判定必须改成「能力探测 + 平台上限」逻辑，否则日志会误报。
> 这条取决于「清单是否总能生效」——属于需要实测确认的点（见第 8 章）。

---

## 4. 推荐路线

**推荐：路线 A（双产物）为主，路线 B 作为「先跑通」的过渡。**

理由：

1. 本项目现有的 PerMonitorV2、Mica、圆角是**刻意做的产品卖点**（见 `app.manifest` 的注释与
   `theme.rs`），为了兼容早已 EOL 的 Win7 而让 Win10/11 用户降级，性价比不对；
2. nightly + `build-std` 属于「游走在 rustc 支持边缘」的构建方式，把它关在一条独立 workflow 里，
   即使哪天 win7 目标被 rust-lang 移除，也只是砍掉一条产品线，不会让主线编译不出来；
3. 维护成本可控：两种产物共用同一份源码，差异只在 `Cargo.toml` 特性开关、`.cargo/config.toml`
   的 target 段、以及 `src/dpi.rs` 的能力探测逻辑（两者都能在同一个二进制里共存）。

实施节奏建议：

- **第一阶段**：按路线 B 把 Win7 跑通（改动最小、能最快拿到「可在 Win7 启动」的证据）；
- **第二阶段**：把 DPI 能力探测改成与构建目标无关的运行时探测（本来就是这么设计的），
  恢复 Win10+ 产物的 PerMonitorV2 路径，形成路线 A 的双产物。

---

## 5. 实施步骤

### 步骤 1：构建工具链与依赖特性

1. **移除 `Win32_UI_HiDpi` 特性**（`Cargo.toml`）。它是唯一引入 `api-ms-win-shcore-scaling-l1-1-1.dll`
   的特性，同时携带 5 个 Win10 才有的 user32 入口点。
   保留 `Win32_Graphics_Gdi`（后面要用 `GetDeviceCaps`）。

2. **加上 `windows_slim_errors` 编译开关**。实测确认（附录 A 探针 2）：该开关会让
   `windows-result 0.2.0` 完全不引用 `RoOriginateErrorW`，产物里的
   `api-ms-win-core-winrt-error-l1-1-0.dll` 随之消失。代价是失去 WinRT 扩展错误信息
   （本项目的错误处理只用 `HRESULT` + `FormatMessage`，不依赖它）。

   ```toml
   # .cargo/config.toml 新增
   [target.x86_64-win7-windows-gnu]
   rustflags = ["--cfg", "windows_slim_errors"]

   [unstable]
   build-std = ["std"]
   ```

3. **改用 win7 目标 + `build-std`**。已实测 `x86_64-win7-windows-gnu` 与
   `x86_64-win7-windows-msvc` 都在 `rustc --print target-list` 中，但 `rustup target add` 会报
   `has no prebuilt artifacts available` —— 必须 `-Z build-std` 现场编译 std，因此需要 nightly。

   ```bash
   cargo +nightly-x86_64-pc-windows-gnu build --release \
       -Z build-std --target x86_64-win7-windows-gnu
   ```

4. **锁定 nightly 版本**（`rust-toolchain.toml`），避免「今天能编、下周不能编」：

   ```toml
   [toolchain]
   channel = "nightly-2026-10-27-x86_64-pc-windows-gnu"
   components = ["rust-src"]
   ```
   注意不要写 `targets = [...]`——tier-3 目标没有预编译 std，写了会直接报错。

5. **验收本步骤**：用探针（附录 A 探针 1）确认 std 侧 4 个问题全部消失。

### 步骤 2：新增 `src/dpi.rs`，用能力探测替代静态导入

这是源码侧的核心改动。原则：**能力探测（`GetProcAddress` 是否成功）优先于版本判断**，
这样同一份代码在 Win7/8/10/11 上都走最短的正确路径。

需要取代的 6 个函数：

| 原函数 | 最低系统 | 运行时探测替代 | Win7 回退实现 |
|---|---|---|---|
| `GetDpiForSystem` | Win10 1607 | `user32!GetDpiForSystem` | `GetDeviceCaps(GetDC(NULL), LOGPIXELSX)` |
| `GetDpiForWindow` | Win10 1607 | `user32!GetDpiForWindow` | 退回 `system_dpi()` |
| `GetDpiForMonitor` | Win8.1 | `shcore!GetDpiForMonitor` | 退回 `system_dpi()` |
| `SetProcessDpiAwarenessContext` | Win10 1703 | `user32!...` | 见下 |
| `SetProcessDpiAwareness` | Win8.1 | `shcore!...` | 见下 |
| `GetThreadDpiAwarenessContext` / `GetAwarenessFromDpiAwarenessContext` | Win10 1607 | `user32!...` | `GetProcessDPIAware()` 映射为 0/1 |

- `SetProcessDPIAware()`（`user32`，Vista+）**可以静态导入**，作为三级回退兜底。它是本项目
  唯一新增的静态导入，Win7 一定存在。为了保持「零新增依赖」，用裸 `extern` 声明即可：

  ```rust
  #[link(name = "user32")]
  unsafe extern "system" {
      fn SetProcessDPIAware() -> i32; // BOOL
  }
  ```
- `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`、`PROCESS_PER_MONITOR_DPI_AWARE`、
  `MDT_EFFECTIVE_DPI`、`ERROR_ACCESS_DENIED` 这些都是**常量**，与特性无关，自行定义为
  常量或用 `GetProcAddress` 的符号名即可。
- 函数指针缓存到 `OnceLock`，只解析一次。

### 步骤 3：替换 10 个调用点

保持现有函数签名不变以减少改动面（`util::dpi_of(hwnd)` 等），内部改为转发到 `dpi.rs`：

> 下表的行号取自编写本方案时的工作区快照。工作区可能正在被并发编辑（例如
> `src/cursor.rs` 的存废），**实施前请先按符号名重新 grep 一遍**，不要照抄行号。
> 若 `src/cursor.rs` 已不存在，对应两行直接删除即可（该文件只是 `GetDpiForMonitor` 的调用方之一）。

| 文件:行 | 当前 | 改为 |
|---|---|---|
| `src/util.rs:22,77` | `HiDpi::GetDpiForWindow` | `dpi::dpi_of(hwnd)` |
| `src/theme.rs:30,295` | 同上 | 同上 |
| `src/toolbar.rs:107` | `HiDpi::GetDpiForSystem` | `dpi::system_dpi()` |
| `src/toast.rs:18,84` | 同上 | 同上 |
| `src/menu.rs:20,172,222` | 同上 | 同上 |
| `src/overlay.rs:522` | 同上 | 同上 |
| `src/pin.rs:41,78` | `HiDpi::GetDpiForMonitor` | `dpi::dpi_of_monitor(hmon)` |
| `src/cursor.rs:22,90` | 同上 | 同上 |
| `src/main.rs:43,144-147` | `SetProcessDpiAwarenessContext` / `SetProcessDpiAwareness` | `dpi::set_dpi_awareness()` |
| `src/main.rs:152-177,337` | `GetAwarenessFromDpiAwarenessContext` / `GetThreadDpiAwarenessContext` | `dpi::current_awareness()` |

⚠️ `src/main.rs:164-177` 的 `verify_dpi_awareness()` 需要同步调整：它现在**硬期望 2（每显示器感知）**，
在 Win7 上永远不满足，会一直打「截图会模糊」的告警——变成噪音。应改为：
`aware >= 平台上限` 即通过，其中平台上限来自能力探测结果（能拿到 PerMonitorV2 → 期望 2；
只能拿到 `SetProcessDPIAware` → 期望 1）。日志文案也要相应区分「平台上限」与「真的退化了」。

### 步骤 4：`SetWindowDisplayAffinity` 的值选择（需要产品决策）

`toolbar.rs:143`、`tooltip.rs:92`、`overlay.rs:256`、`main.rs:313` 使用了
`WDA_EXCLUDEFROMCAPTURE`（`0x11`，Win10 2004+）。这是**常量**，不会导致启动失败，但 Win7 上调用会失败。
可选处理：

- (a) 先试 `0x11`，失败则回退 `WDA_MONITOR`（`0x01`，Win7+）——保留「防止把自己拍进截图」的能力，
  但被第三方截图工具捕获时会呈**黑块**；
- (b) Win7 上**跳过**该调用——不再有黑块，但自己的窗口可能出现在后续截图中（本项目的截图基于
  按下热键时的整屏冻结图，工具栏是冻结之后才出现的，所以实际影响很小）。

**倾向 (b)**，但这属于产品行为取舍，需确认（见第 8 章）。

### 步骤 5：清单补充 `supportedOS`

`app.manifest` 的 `compatibility/application` 目前只有 Win10 的 GUID。按 Microsoft 文档补全：

```xml
<supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/> <!-- Windows 7 / Server 2008 R2 -->
<supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/> <!-- Windows 8 / Server 2012 -->
<supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/> <!-- Windows 8.1 / Server 2012 R2 -->
```

**`dpiAware` / `dpiAwareness` 不要动**，现有写法 `true/PM` 在 Win7 上已正确降级为系统感知。

### 步骤 6：构建、CI 与发布

1. 新增一条构建链（`--target x86_64-win7-windows-gnu` 或 CI 上更顺手的
   `x86_64-win7-windows-msvc`，Windows runner 自带 MSVC，而本机没有 MSVC 链接器 —— 已实测
   `link.exe` 缺失，本机必须走 GNU）。
2. 产物命名区分，如 `pixshot-lite-v<ver>-x64-win7.exe` 与 `pixshot-lite-v<ver>-x64.exe`；
   或按路线 B 只出 `-win7` 一份。
3. **重新基线体积护栏**。`-Z build-std` 现场编译的 std 与预编译版体积不一定相同，
   现有 `ci.ps1` 的 2.0 MB 与 `release.yml` 的 3.0 MB 上限需要重测后调整。
4. 更新 `README.md` 与 Release 说明：最低系统要求从「Windows 10 1809+」改为
   「**Windows 7 SP1 及以上**」，并列出第 2.4 节的功能降级表。

---

## 6. 风险与规避措施

| # | 风险 | 影响 | 规避措施 |
|---|---|---|---|
| 1 | `x86_64-win7-*` 是 **tier-3 目标**，无官方 CI、可能被 rust-lang 移除或改坏 | 某天构建突然失败 | 锁定 nightly **日期**（`rust-toolchain.toml`）；把该构建链隔离在独立 workflow；在 CI 里加一个「能编译 + 导入表干净」的定时巡检；作为兜底，可长期自持一份打了补丁的 std（成本高，仅作最后手段） |
| 2 | 引入 nightly 工具链，可复现性与供应链风险上升 | 构建环境不可控 | 锁定具体 nightly 日期而非 `nightly` 浮动；`Cargo.lock` 已提交并 `--locked`；`build-std` 只在本构建链启用，不影响主线 |
| 3 | `build-std` 显著增加构建时长（首次约 1 分钟量级，冷缓存更久） | CI 变慢 | CI 缓存 `~/.rustup` 与 `target/`；win7 构建只在打 tag/手动触发时跑 |
| 4 | 无 Win7 实机/虚拟机，验收做不实 | 可能「编出来了但跑不起来」 | **必须先准备 Win7 SP1 x64 环境**（见第 8 章）；GitHub 托管 runner 跑不了 Win7，只能本地 VM |
| 5 | Win7 上只有系统级 DPI，多显示器混合缩放坐标不准 | 功能不正确（非崩溃） | 在文档中明确该限制；`dpi.rs` 里对每显示器 API 不可用的情况记录诊断日志，便于定位 |
| 6 | Win7 关掉 Aero / 用经典主题时 DWM 合成不可用 | Mica/圆角/描边无效 | 现有代码已 `let _ =` 忽略失败，无需改动；工具栏/贴图是分层窗口，不依赖合成 |
| 7 | 仍有 **动态调用**（COM 激活）类依赖不会被导入表暴露 | 静态闸门可能漏检 | 人工复核所有 `CoCreateInstance` / `CLSID`（当前只有 WIC，Win7 自带）；把「新引入的 COM 组件」写入评审清单 |
| 8 | 改动 DPI 逻辑可能重新引入「截图模糊」这个历史顽疾 | 核心功能退化且难以察觉 | 保留并强化 `verify_dpi_awareness()` 的诊断日志；在 Win7 与 Win10 上都要看 `pixshot_diag.log` 的这一行 |
| 9 | `windows_slim_errors` 会改变错误信息能力 | 排障信息变少 | 本项目错误路径只用 `HRESULT` + `FormatMessageW`，实测不受影响；在 `diag.rs` 里确认无依赖 |

---

## 7. 验证与验收

### 7.1 静态闸门（可自动化，必须进 CI）

对产物做导入表断言，任何黑名单命中即失败：

```powershell
# Windows / dumpbin
$bad = @('api-ms-win-', 'bcryptprimitives', 'WaitOnAddress', 'WakeByAddress',
         'ProcessPrng', 'GetSystemTimePreciseAsFileTime', 'GetHostNameW',
         'RoOriginateErrorW', 'GetDpiForWindow', 'GetDpiForSystem',
         'SetProcessDpiAwarenessContext', 'GetThreadDpiAwarenessContext',
         'GetAwarenessFromDpiAwarenessContext')
$out = dumpbin /imports target\x86_64-win7-windows-gnu\release\pixshot.exe
$hit = $out | Select-String -Pattern ($bad -join '|')
if ($hit) { throw "导入表含 Win8+ 依赖：`n$hit" }
```

```bash
# Git Bash / objdump 等价写法
objdump -p target/x86_64-win7-windows-gnu/release/pixshot.exe \
  | grep -E "api-ms-win|bcryptprimitives|WaitOnAddress|WakeByAddress|ProcessPrng|GetSystemTimePreciseAsFileTime|GetHostNameW|RoOriginateErrorW|GetDpiFor[WS]|SetProcessDpiAwareness|ThreadDpiAwareness" \
  && exit 1 || echo "✔ 导入表干净"
```

**本方案已用此方法验证过最终形态是可达的**（附录 A 探针 3）：导入表只剩
`KERNEL32 / advapi32 / dwmapi / kernel32 / msvcrt / ntdll / shell32 / user32 / userenv / ws2_32`，
零 api-set、零 Win8+ 入口点。

### 7.2 构建闸门

`cargo +<pinned-nightly> build --release -Z build-std --target x86_64-win7-windows-gnu`
必须零错误通过（本方案已在探针工程验证该命令可用）。

### 7.3 功能验收（Win7 SP1 x64 环境）

逐项手工验收：

1. 双击启动 → 常驻托盘、无窗口、无报错弹窗；
2. `pixshot_diag.log` 中 `[boot] DPI 感知级别` 行为「符合平台上限」且**不告警**；
3. F1 全局热键触发截图 → 整屏冻结 → 十字光标框选 → 红框保留；
4. 工具栏四个按钮：贴图 / 保存 / 复制并关闭 / 关闭，全部生效；
5. 贴图窗口：左键拖动、滚轮以光标为锚点缩放、右键菜单（复制/保存/重置缩放/关闭）、Esc 关闭；
6. 保存 PNG / JPEG / BMP 三种格式，文件名 `截图_YYYYMMDD_NN` 递增正确；
7. 复制到剪贴板 → 粘贴到画图应得到正确图像；
8. 托盘右键「开机启动」与「退出」生效；
9. Esc 取消截图、单击不产生选区、过小拖动不产生选区；
10. **125% / 150% 系统缩放**下截图清晰、无重采样模糊（这是本方案最容易翻车的点）；
11. 双显示器场景（同缩放）坐标正确；**混合缩放**容忍定位偏差并在文档中标注；
12. `capture_cursor = true` 时鼠标指针出现在截图里且大小合理。

### 7.4 回归验收（Win10 / Win11）

现有 `pwsh -File scripts\ci.ps1`（含 GUI E2E）必须全绿；并**人工确认 Win10/11 上
PerMonitorV2 依然生效**（若采用路线 A 双产物，则验证 `-x64` 产物；若采用路线 B，
需按第 8 章的待确认项先实测清单是否仍能定死 PerMonitorV2）。

### 7.5 体积与发布

重新测量两种产物体积，调整 `ci.ps1` 的 `-MaxSizeMB` 与 `release.yml` 的体积护栏；
Release 说明更新系统要求与降级说明。

---

## 8. 需要补充的信息 / 待决策项（请勿假设）

> ### 实施记录（2026-10-29，本章问题已答复，改造已落地）
>
> 已确定的决策：**按本文档实施**；产物形态取**路线 B（单产物）**；**接受 nightly + build-std**；
> Win7 上 `SetWindowDisplayAffinity` 取 **(b) 跳过调用**（有降级需求时消费方是第三方截图工具，
> 用 `WDA_MONITOR` 的黑块比不设防更影响观感）。
>
> 实施中发现并偏离本文档的 4 处（均已实测）：
>
> 1. **工具链文件无法同时表达「日期 + gnu 宿主」**：rustup 拒绝
>    `<channel>-<发布日期>-<宿主三元组>` 的写法（报 `target tuple in channel name`），
>    而只写 `nightly-2026-10-27` 会落到 msvc 宿主、本机没有 MSVC 链接器直接编不过。
>    故 `rust-toolchain.toml` 只写浮动名 `nightly-x86_64-pc-windows-gnu`，
>    **日期锁定改在发布链上表达**（`.github/workflows/release.yml` 用
>    `nightly-2026-10-27-x86_64-pc-windows-gnu` 完整名）。
> 2. **第 2 章「唯一新增的静态导入 `SetProcessDPIAware`」不成立**：MinGW 的 `libuser32.a`
>    里根本没有 `GetProcessDPIAware`，静态导入在链接期就报 `undefined reference`
>    （与 std 注释里提到的"老 mingw 导入库"问题同源）。故 `src/dpi.rs`
>    **不再有任何静态导入**，连 `SetProcessDPIAware` / `GetProcessDPIAware` 也走 `GetProcAddress`。
> 3. **步骤 1 的 `[unstable] build-std` 不能放进 `.cargo/config.toml`**：该节对所有构建生效，
>    会让日常 `cargo check` / `test` 也走 build-std（慢且强制 nightly）。
>    改为只在发布构建与 CI 的命令行上传 `-Z build-std`。
> 4. **体积基线变了**：`-Z build-std` 现场编译的 std 比预编译版大，
>    产物 1.6 MB → **1.93 MB**；`scripts/ci.ps1` 的体积护栏 2.0 MB → **2.5 MB**。
>
> 仍未验证的部分（第 7.3 节的功能验收）：**没有 Windows 7 SP1 x64 环境**，
> 目前只验证到「win7 目标编译通过 + 导入表静态闸门通过 + 在 Windows 10 上冒烟与 E2E 通过」。
> Win7 上的 DPI 回退分支（系统感知路线）与 WDA 跳过分支**未经真机执行**。

以下信息缺失会直接影响方案落地，**在得到答复前不应开始实施**：

1. **是否允许引入 nightly 工具链 + `build-std`？** 这是全局最关键的一个决策。
   已实测这是**唯一可行**的技术路径（第 3 章已排除垫片 DLL 与旧工具链两条路）。
   如果不能接受 nightly，「支持 Win7」这个目标在现有代码基上无法达成。
2. **是否有 Win7 SP1 x64 的测试环境（虚拟机/实体机）？** 没有的话第 7.3 节的功能验收无法执行，
   「支持 Win7」只能停留在「能编译、导入表干净」的程度，不能声称可用。
3. **目标 Win7 的具体版本与位数**：SP1 是必需的（`SetWindowDisplayAffinity` 等要求 Win7+）；
   是否还需要 **32 位（i686）** 产物？若需要，`i686-win7-windows-gnu` 同样存在，但要多一条构建链
   与一轮验收。
4. **「Windows 7 及以上」是否需要单独覆盖 Windows 8 / 8.1？** 同一个 win7 目标可以运行在
   8/8.1 上，但 8.1 上已经存在 `SetProcessDpiAwareness` / `GetDpiForMonitor`（Win8.1 才有），
   能力探测会让它走「每显示器感知」路径 —— 这是一个**和 Win7 不同的代码路径**，需要单独验收。
5. **分发形态**：单产物（路线 B）还是双产物（路线 A）？若双产物，产物命名与 Release 组织方式
   如何安排（当前是「一个 version = 一个 tag = 一个 exe」的模型）。
6. **第 5 章步骤 4 的产品取舍**：Win7 上 `WDA_EXCLUDEFROMCAPTURE` 不可用时，工具条/遮罩/贴图窗口
   的防捕获策略选 (a) 回退 `WDA_MONITOR`（黑块）还是 (b) 不设防？我倾向 (b)。
7. **第 4 章路线 B 的待实测项**：需要在一台 Win10 1703+ 机器上确认「仅靠 `app.manifest` 的
   `PerMonitorV2` 声明、不做运行时 `SetProcessDpiAwarenessContext` 调用」是否仍能得到每显示器
   感知。若成立，路线 B 的能力退化比预期小；若不成立，路线 B 必须牺牲 Win10/11 的 PerMonitorV2。
8. **Win7 上的观感降级是否可接受**（无 Mica、无圆角、无深色标题栏）？若不可接受，
   需要额外设计一套 Win7 专用配色（`theme.rs` 的 `MICA 开关` 已有回退为实色的机制）。

---

## 附录 A：复现步骤

以下探针工程均在临时目录中创建，不污染本仓库。

### 探针 1：验证「std 侧 4 个 Win8+ 依赖」与「win7 目标可消除」

```bash
# 纯 std 探针（用到 thread/Mutex/Condvar/RwLock/OnceLock/HashMap/Instant/SystemTime/net）
cargo build --release                                   # 默认 x86_64-pc-windows-gnu
objdump -p target/release/win7probe.exe | grep -E "api-ms-win|bcryptprimitives"
# → 命中 api-ms-win-core-synch-l1-2-0.dll + bcryptprimitives.dll
# → 入口点命中 WaitOnAddress / WakeByAddressAll / WakeByAddressSingle / ProcessPrng
#            / GetSystemTimePreciseAsFileTime / GetHostNameW

cargo +nightly-x86_64-pc-windows-gnu build --release \
    -Z build-std --target x86_64-win7-windows-gnu
objdump -p target/x86_64-win7-windows-gnu/release/win7probe.exe | grep "DLL Name"
# → ADVAPI32 / KERNEL32 / USERENV / WS2_32 / msvcrt / ntdll，无 api-set
# → 上述入口点全部消失
```

注：`rustup target add x86_64-win7-windows-gnu` 会失败并提示
`has no prebuilt artifacts available for target`，因此必须 `-Z build-std`。
另：`nightly` 默认装的是 msvc 宿主，本机无 MSVC 链接器（实测 `link.exe` 缺失），
必须用 `nightly-x86_64-pc-windows-gnu`。

### 探针 2：验证 `windows_slim_errors` 能去掉 WinRT 导入

```bash
cargo build --release                                  # 含 Win32_UI_HiDpi
objdump -p target/release/win7probe2.exe | grep "api-ms-win"
# → api-ms-win-core-synch-l1-2-0.dll, api-ms-win-core-winrt-error-l1-1-0.dll

RUSTFLAGS="--cfg=windows_slim_errors" cargo build --release --target-dir target_slim
objdump -p target_slim/release/win7probe2.exe | grep "api-ms-win"
# → 只剩 api-ms-win-core-synch-l1-2-0.dll（std 侧，需 win7 目标解决）
#    winrt-error 已被消除
```

### 探针 3：验证「最终形态」导入表干净

工程配置 = pixshot 当前的 `windows` 特性集合 **减去 `Win32_UI_HiDpi`**，
编译参数 = `RUSTFLAGS="--cfg=windows_slim_errors"` + win7 目标 + `-Z build-std`。
结果：

```
DLL Name: KERNEL32.dll / advapi32.dll / dwmapi.dll / kernel32.dll / msvcrt.dll
DLL Name: ntdll.dll / shell32.dll / user32.dll / userenv.dll / ws2_32.dll
✔ 无任何 api-set / Win8+ 专属导入
```

**这就是本方案的目标状态。** 剩余工作是把应用源码里对 HiDpi 的 10 处引用改成运行时探测
（第 5 章步骤 2、3），并补齐清单/文档/CI。

### 查证过的外部事实

- `<dpiAware>true/PM</dpiAware>` 在 Windows Vista/7/8 上 → **系统 DPI 感知**；
  在 Windows 8.1/10 上 → 每显示器 DPI 感知
  （Microsoft Learn《Application manifests》的 `dpiAware` 行为表）。
- `supportedOS` 各版本 GUID（同上文档 `supportedOS` 段）。
- KnownDLLs 列表（本机注册表实查）：含 `kernel32`、`user32`、`ws2_32`、`shcore`、`gdiplus`、
  `ole32`、`oleaut32`、`gdi32`、`advapi32`、`comdlg32`、`shell32`、`imm32`、`msvcrt`。
