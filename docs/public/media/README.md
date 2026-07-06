# 演示素材 / Demo media

把 GIF / 视频演示素材放在这个文件夹,在文档中用站点根路径引用,例如:

Put GIF / video demo assets here and reference them from docs with a
site-root path, e.g.:

```md
<!-- 推荐:视频体积更小、更清晰 / Prefer video — smaller & sharper -->
<video src="/media/demo.mp4" autoplay loop muted playsinline />

<!-- 或 GIF / or GIF -->
![演示](/media/demo.gif)
```

大文件建议走 Git LFS,避免撑大仓库。
For large files, consider Git LFS to avoid bloating the repo.
