# ttyticker

> 老实看监控，很枯燥吧？

把系统指标做成证券行情终端的终端版。CPU、GPU、内存、功耗每一项都是一只「个股」，
有交易所风格的代码、实时 K 线、均线、量柱；进程是成分股，按属主聚合成龙虎榜。

灵感来自 [PerfTicker](https://github.com/QiuSui1145/PerfTicker/)（Windows GUI 版），
K 线渲染参考 [tickrs](https://github.com/tarkah/tickrs) 的 Canvas 几何。

## 界面

```
┌ 标题栏  芯片 · 核心数 · 采样周期 · 配色 · 焦点提示 ────────────────┐
├ 指数条  八格：CPU 活跃 / 频率 / GPU / 内存 / 温度 / 功耗 ──────────┤
├───────────┬────────────────────────────┬─────────────────────────┤
│ 大盘/板块  │ 品种头  代码 名称 现价 涨跌  │ 主力动向  Top 10 进程   │
│ 行情       │ K 线主图 + 均线 + 量柱      │ 逐笔成交  事件流水      │
│ 28 项      │ 右侧价格轴                 │                         │
├───────────┴────────────────────────────┴─────────────────────────┤
│ 龙虎榜  资源占用前十的席位 │ 个股列表  进程表，按席位可过滤       │
├──────────────────────────────────────────────────────────────────┤
│ 状态栏  采样 · 运行 · 样本 · 进程 · 按键提示                     │
└──────────────────────────────────────────────────────────────────┘
```

## 系统要求

| | 要求 |
|---|---|
| 工具链 | Rust **1.85 或更高**（`edition = "2024"`），`cargo build` 会检查 |
| macOS | **Apple Silicon**（M1 及以上）。macmon 走 IOReport 私有框架，Intel 芯片拿不到功耗与温度 |
| Linux | x86_64 或 aarch64，内核 4.x 以上。只需要 `/proc` 与 `/sys`，无第三方运行时依赖 |

**不需要 root**。macOS 侧免 sudo 拿功耗，Linux 侧全部是只读查询。

## 安装工具链

没装 Rust 的话：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/env
```

国内网络建议配镜像，否则拉依赖会非常慢（实测官方源单次请求要 8 秒，rsproxy 只要 0.5 秒）：

```bash
mkdir -p ~/.cargo
cat > ~/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
EOF
```

## 编译

```bash
cargo build --release
```

产物在 `target/release/ttyticker`，约 1.7 MB，无动态依赖。

两个平台的依赖是分开的，`macmon` 只在 macOS 上引入：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
macmon = { version = "0.10.0", default-features = false }
```

所以在 Linux 上编译不会碰到 Apple 框架，反之亦然。

## 运行

```bash
./target/release/ttyticker
```

需要交互式终端。SSH 到服务器时先 `ssh host` 进去再启动，直接 `ssh host ./ttyticker` 拿不到 TTY。

最小可用宽度 100 列，窄于此会提示。

## 按键

| 键 | 作用 |
|---|---|
| `Tab` | 三区循环切换焦点：板块 → 龙虎榜 → 个股 |
| `↑` `↓` / `k` `j` | 在当前区移动选中行 |
| `回车` | 龙虎榜：按席位过滤进程表；个股：把主图锁到该进程 |
| `空格` | 暂停 / 继续采样 |
| `1`–`5` | 切换 K 线周期（2 秒 / 5 秒 / 10 秒 / 30 秒 / 1 分） |
| `C` | 红涨绿跌 ↔ 绿涨红跌 |
| `M` | 均线档位：三条 → 只 MA5 → 关闭 |
| `S` | 个股列表排序：CPU → 内存 → 线程 → PID |
| `/` | 搜索进程名或 PID，`Esc` 清空 |
| `Q` | 退出 |

## 诊断模式

不进交互界面，跑完直接输出，用于回归验证。

| 命令 | 用途 |
|---|---|
| `--once` | 采样若干轮后导出一帧带颜色的 ANSI，重定向到文件即可看渲染结果 |
| `--demo` | 同上，但注入多属主的样例进程，验证席位显示 |
| `--faults-probe` | 连续采样，统计换页速率与内存占用，输出与 CPU 占用的相关系数 |
| `--grid-test` | 探测 ratatui Canvas 对 Rectangle / Line 的栅格化行为 |

`--once` 与 `--demo` 支持这些环境变量：

| 变量 | 含义 |
|---|---|
| `PT_ROUNDS` | 采样轮数，默认 180 |
| `PT_COLS` / `PT_ROWS` | 渲染尺寸，默认 150 × 42 |
| `PT_MARKER` | Canvas 栅格化方案：`braille`（默认）/ `block` / `halfblock` / `dot` |
| `PT_SELECT` | 指定品种代码，如 `000001`，看它的副图口径 |
| `PT_LOCK_SELF` | 主图锁定到本进程 |
| `PT_LOCK_TOP` | 主图锁定到当前占用最高的进程 |
| `PT_DEBUG_CHART` | 打印主图的布局数值（容量、根数、偏移） |
| `PT_DEBUG_VM` | 打印 host_statistics 的原始返回 |

导出后去掉颜色码：

```bash
./target/release/ttyticker --once | python3 -c "import re,sys; print(re.sub(r'\x1b\[[0-9;]*m','',sys.stdin.read()))"
```

## 数据来源

| 指标 | macOS | Linux |
|---|---|---|
| CPU 占用（整机 + 逐核） | macmon（IOReport） | `/proc/stat` 两端差分 |
| CPU 频率 | macmon 频率加权驻留 | `cpufreq/scaling_cur_freq`，回退 `/proc/cpuinfo` |
| GPU 活跃 / 频率 / 功耗 / 温度 | macmon | amdgpu 的 `gpu_busy_percent` |
| 内存与 Swap | macmon | `/proc/meminfo` |
| 换页速率（副图成交量） | `host_statistics` 的 `pageins` | `/proc/vmstat` 的 `pgfault` |
| 功耗 | macmon | RAPL `/sys/class/powercap` |
| 温度与风扇 | macmon（SMC） | `/sys/class/hwmon` |
| 进程与属主 | sysinfo | sysinfo |

## 已知限制

**`vm_statistics.faults` 在现代 macOS 上是废弃字段**，内核恒返回 `-1`。缺页速率改用
`pageins`（`faults` 的子集，只计真正从磁盘换页的 major fault）。

**sysinfo 在 macOS 上不支持线程列表**，`Process::tasks()` 恒为 `None`。要拿线程数得走
`proc_pidinfo(PROC_PIDTASKINFO)`。同一结构里 offset 80 是 `pti_csw`（每进程上下文切换数），
累加即 PerfTicker 在 Windows 上用的那个口径。

**`sysctl kern.num_threads` 不是当前线程数**，实测恒定不变，应该是内核哈希表容量之类的静态量。

**Linux 的 RAPL 在容器与虚拟机里通常不可读**，此时功耗类指标显示 `—` 而不是 `0W`。
`has_gpu` 与 `has_power` 两个能力标记控制这件事。

**NVIDIA 卡在 Linux 上暂不支持**，需要走 NVML（`libnvidia-ml.so`）。目前只覆盖 amdgpu 的 sysfs 接口。

## 架构

```
src/
├─ probe/         采集层，按平台分文件，对外只暴露一个 Probe
│  ├─ mod.rs      平台分发 + UID 解析 + 席位聚合 + 进程刷新
│  ├─ macos.rs    macmon + host_statistics
│  └─ linux.rs    /proc + /sys + RAPL + hwmon
├─ snapshot.rs    一次采样的不可变快照，采集层与上层唯一的契约
├─ catalog.rs     指标目录：代码、单位、精度、能力标记、成交量口径
├─ candle.rs      tick 到 OHLC 的聚合与均线
├─ tape.rs        逐笔成交：告警、跳变、进程启停
├─ theme.rs       TradingView Lightweight Charts 的暗色默认配色
├─ ui/            渲染层
│  ├─ mod.rs      六区布局与各面板
│  └─ chart.rs    K 线主图，Canvas + Braille 栅格化
└─ app.rs         状态机、采样线程、席位过滤
```

采集层只吐 `Snapshot`，界面只认 `MetricId`，两边通过 `catalog` 对话。
新增平台等于新增一个 `probe` 实现，上层一行不动。
