# Pi Web Tray

只在菜单栏或系统托盘运行的 Pi Web 启动器，没有应用窗口。双击启动后，Pi Web 会在 `http://127.0.0.1:30141` 运行，不会自动打开浏览器。需要网页时，点图标 → **在浏览器中打开**。部分 Linux 桌面用右键打开托盘菜单。

**停止服务**保留托盘，**退出 Pi Web Tray**先停止服务再退出。端口已被其他应用占用时不会接管，也不会停止外部进程。重启、停止、退出和更新 Pi Web 都会中断正在执行的任务。

## 运行要求

- Node.js 22.19+ 和 npm。先用 `npm install -g @earendil-works/pi-coding-agent @agegr/pi-web` 安装官方包。同名的无作用域 `pi-web` 不是这个项目。
- 托盘使用当前 `PATH` 中的 Node，另查 Homebrew 等常见路径。同一份 Node/npm 必须能在 `npm root -g` 指向的目录找到全局包。如果依赖只在交互式终端加载的版本管理器，先从该终端运行可执行文件，或使用 GUI 可见的 Node/npm 安装。首版不支持 npx 缓存、npm link 或非 npm 安装。
- Linux 桌面需要 AppIndicator / StatusNotifierItem 托盘支持；部分 GNOME 桌面需要启用 AppIndicator 扩展。只提供 Debian/Ubuntu `.deb` 安装包。

## 检查与更新

托盘分别显示 **Pi CLI** 和 **Pi Web** 的已装版本与 npm 配置源中的 `latest`。仅在点击确认后更新指定版本，不提权；更新 Pi Web 前停止本应用启动的服务，有其他在运行的 Pi Web 实例时拒绝更新。更新完会恢复原先运行的服务；更新失败也会尝试重启，但不会擅自降级或重装。

更新全局 Pi CLI **不会**更新网页内使用的 Pi 依赖；后者随 `@agegr/pi-web` 一起升级。Pi 的配置和会话目录沿用上游默认设置，不迁移数据。服务只监听本机，不提供开机自启或应用自身的自动更新。

## 开发与打包

在本目录运行，需 Rust 1.88+、Node.js 22.19+ 和对应平台的 [Tauri 系统依赖](https://v2.tauri.app/start/prerequisites/)。Windows 使用 MSVC，Linux 需要 GTK3、WebKitGTK 4.1、AppIndicator 等构建依赖（见工作流）。应用不创建 WebView，Windows 安装包也不下载 WebView2。

```text
cargo test --locked --manifest-path src-tauri/Cargo.toml
npx --yes @tauri-apps/cli@2.12.6 build --bundles dmg -- --locked    # macOS
npx --yes @tauri-apps/cli@2.12.6 build --bundles nsis -- --locked   # Windows
npx --yes @tauri-apps/cli@2.12.6 build --bundles deb -- --locked    # Ubuntu/Debian
```

三个包分别在目标系统上构建，默认输出到 `src-tauri/target/release/bundle/`；GitHub Actions 的构建结果在该次运行的 Artifacts 中下载。macOS 安装包未做 Developer ID 签名或公证，Windows 安装包未做 Authenticode 签名。

应用与托盘共用根目录的 `icon.svg`；macOS 菜单栏也保留原色。换图后运行 `npx --yes @tauri-apps/cli@2.12.6 icon icon.svg --output <临时目录>`，更新 `src-tauri/icons/` 中已有的四个文件，再重新打包。

### 桌面验收

1. 从 Finder、开始菜单或 Linux 桌面启动：只显示托盘图标，不自动打开网页；打开状态菜单应显示运行中。再次启动不应新增实例。
2. 从托盘打开网页；停止后验证端口关闭，重新启动和重启后端口恢复；退出后图标消失且没有残留 Next.js 进程。
3. 用外部程序占用 `127.0.0.1:30141` 再启动：报告端口冲突，外部程序不受影响。
4. 分别检查两个包的更新。使用一次性 npm prefix 安装较旧版本验证取消、成功更新、网络或权限失败；不要拿日常使用的全局安装做降级测试。
