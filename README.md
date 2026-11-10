# win-pin

一个 68×68 的窗口置顶图钉。把图钉拖到任意窗口上松开，那个窗口就置顶。

只为这一件事存在，所以它只有几 MB，常驻内存二十出头，不装运行库，双击即用。

## 用法

| 操作 | 行为 |
| --- | --- |
| 按住图钉拖到某个窗口上松手 | 那个窗口置顶，图钉变红并左倾 |
| 单击图钉 | 解除置顶，图钉恢复灰蓝直立 |
| 按住图钉拖到桌面空白处松手 | 不钉任何东西，图钉就留在那儿（挪到顺手的位置） |
| 右键图钉 | 菜单：解除置顶 / 退出 |
| 被钉的窗口关掉 | 图钉自动松开 |

位置会记在 `%APPDATA%\win-pin\`（首次启动在屏幕右下角，距边缘 26px）。

## 为什么不用 PowerToys

PowerToys 的 Always On Top 要装整个套件，常驻几个进程、几百 MB。这个功能本身只需要一次
`SetWindowPos(HWND_TOPMOST)`，用不着那样。

## 实现要点

- **找出鼠标下是哪个窗口**：不用 `WindowFromPoint`。拖动时图钉就压在鼠标正下方，
  `WindowFromPoint` 只会返回图钉自己。改成从 Z 序最顶层往下扫，逐个比对窗口矩形，
  跳过自己、跳过鼠标穿透层、跳过桌面外壳类（Progman / WorkerW / 任务栏）。
- **区分单击和拖动**：系统拖动（`ViewportCommand::StartDrag`）结束后，egui 的
  `outer_rect` 还是旧值，拿它当判据会把拖动误判成单击。改用 `GetCursorPos`
  比较鼠标位移，即时可靠。
- **右键菜单用 Win32 原生菜单**：egui 的 `context_menu` 画在当前视口内部，
  68px 的窗口会把菜单整块裁掉。`TrackPopupMenu` 不受窗口边界限制。
- **窗口裁圆**：`SetWindowRgn` + `CreateEllipticRgn`。顺带让四个角的透明区域
  不再拦截鼠标，否则图钉旁边会有一圈方形热区挡住桌面。
- **去掉 DWM 描边**：`DWMWA_BORDER_COLOR = NONE`，同时把 winit 给透明窗口设的
  玻璃框架延伸归零（`DwmExtendFrameIntoClientArea`，margins 全 0）—— 否则
  客户区顶部会多出一条 1px 的框架线。

## 低功耗

- 没有钉住任何窗口、也没有动画时，完全不调用 `request_repaint`，CPU 归零；
- 悬停、倾斜、拖动这些过渡只按 16ms 重绘，进程静止后自动挂起；
- 钉住时每秒检查一次目标窗口是否还活着，仅此一次唤醒。

## 构建

本地不需要装任何东西，推送到 GitHub 后由 Actions 编译：

```
Actions → build-windows-exe → 取 win-pin-windows-x64 产物
```

产物是单个 exe，静态链接 CRT，目标机器不需要 VC++ 运行库。

本地构建（需 rustup + MSVC Build Tools）：

```
cargo build --release
```
