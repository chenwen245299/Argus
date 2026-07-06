# 安装教程

Argus 是一款**轻量、AI 原生、本地优先**的桌面应用,支持 macOS 和 Windows。本页介绍如何下载并安装它。

## macOS

1. 从 [下载](/zh/download) 页(或 [Releases](https://github.com/chenwen245299/Argus/releases)页面)下载最新的 `.dmg`并安装。
2. 由于我没钱买苹果的开发者账号，所以没有证书，导致macOS 首次打开会被拦截。针对该问题，只需要在终端里运行下面的代码清除隔离标记即可:

   ```bash
   xattr -cr /Applications/Argus.app
   ```
3. 从「应用程序」启动 Argus。

## Windows

从 [下载](/zh/download) 页下载最新安装包(`.msi` / `.exe`)并运行。

## 从源码构建

需要 Node.js 22+ 和 Rust stable 工具链。

```bash
npm install          # 安装依赖(同时复制 Vditor 资源)
npm run tauri dev    # 以开发模式运行桌面应用
npm run tauri build  # 为当前平台构建生产安装包
```

## 下一步

装好了?继续 [快速上手](/zh/guide/quick-start)。
