# 连接 Argus Token 用量工具

1. 使用包含新工具的 Argus，在 **设置 → MCP 接口** 中开启开关。
2. 另一个 App 添加 **STDIO MCP 服务器**，配置如下：

```json
{
  "mcpServers": {
    "argus": {
      "command": "/Applications/Argus.app/Contents/MacOS/Argus",
      "args": ["--mcp-stdio"]
    }
  }
}
```

`command` 换成 Argus MCP 设置页显示的可执行文件路径。默认读取 Argus 最近打开的文献库；要固定文献库，可给该服务器添加环境变量 `ARGUS_LIBRARY=/你的文献库绝对路径`。

3. 连上后调用 `get_token_usage`，例如查询新加坡时间 10 月 20 日的用量：

```json
{
  "name": "get_token_usage",
  "arguments": {
    "start_time": "2026-10-20T00:00:00+08:00",
    "end_time": "2026-10-21T00:00:00+08:00",
    "include_records": false
  }
}
```

通过 MCP 客户端调用后，从结果的 `structuredContent.summary` 读取汇总。传 `arguments: {}` 可以查询全部历史。

这是同一台电脑上的连接方式；其他设备需要电脑上的程序转发。
