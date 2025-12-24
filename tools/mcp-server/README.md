# DragonOS MCP服务器

DragonOS MCP服务器是一个Model Context Protocol (MCP)服务器，使AI助手（如Cursor、Claude Code）能够与DragonOS虚拟机进行自动化交互。

## 功能特性

- **虚拟机管理**：启动、停止、查询DragonOS虚拟机状态
- **串口交互**：读取虚拟机输出，支持多客户端同时连接
- **测试执行**：在虚拟机内自动执行测试用例
- **GDB调试**：自动获取调用栈，分析死锁和等待问题
- **开发者友好**：支持开发者同时连接终端进行手动交互

## 重要说明

**MCP服务器的限制**：由于权限问题，MCP服务器**只会编译内核并启动虚拟机**，不会：
- 编译用户程序（`make user`）
- 写入rootfs镜像（`make write_diskimage`）

如果需要更新用户程序或rootfs镜像，请手动运行：
```bash
make all && make write_diskimage
```

然后MCP服务器可以启动已更新的虚拟机。

## 安装

### 1. 安装Python依赖

```bash
cd tools/mcp-server
pip install -r requirements.txt
```

或者使用虚拟环境：

```bash
python -m venv venv
source venv/bin/activate  # Linux/Mac
# 或 venv\Scripts\activate  # Windows
pip install -r requirements.txt
```

### 2. 配置Cursor

将以下配置添加到Cursor的MCP服务器配置中（通常在用户配置目录）：

```json
{
  "mcpServers": {
    "dragonos": {
      "command": "python",
      "args": [
        "-m",
        "dragonos_mcp.server"
      ],
      "env": {
        "DRAGONOS_ROOT": "${workspaceFolder}",
        "PYTHONPATH": "${workspaceFolder}/tools/mcp-server/src"
      }
    }
  }
}
```

配置文件位置：
- Linux/Mac: `~/.config/Cursor/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`
- Windows: `%APPDATA%\Cursor\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json`

## 使用方法

### AI助手使用

AI助手可以通过MCP工具调用以下功能：

1. **启动虚拟机**：`dragonos_vm_start`
2. **停止虚拟机**：`dragonos_vm_stop`
3. **查询状态**：`dragonos_vm_status`
4. **读取输出**：`dragonos_serial_read`
5. **等待启动**：`dragonos_wait_boot`
6. **执行测试**：`dragonos_test_run`
7. **获取调用栈**：`dragonos_gdb_backtrace`
8. **分析卡死**：`dragonos_gdb_analyze`

### 开发者终端连接

当AI助手通过MCP运行DragonOS时，开发者可以同时连接到虚拟机终端：

```bash
# 方法1：使用make命令
make connect-serial

# 方法2：直接使用socat
socat - UNIX-CONNECT:$(pwd)/bin/tmp/hypervisor/serial-x86_64.sock

# 方法3：查看socket路径
make serial-path
```

## 目录结构

```
tools/mcp-server/
├── pyproject.toml          # Python项目配置
├── requirements.txt        # 依赖项
├── README.md              # 本文档
├── src/
│   └── dragonos_mcp/
│       ├── server.py      # MCP服务器主程序
│       ├── mcp/           # MCP协议实现
│       ├── qemu/           # QEMU管理
│       ├── serial/         # 串口交互
│       ├── gdb/            # GDB集成
│       └── test/           # 测试执行
└── scripts/
    └── connect-serial.sh   # 开发者连接脚本

bin/tmp/hypervisor/        # 运行时目录（自动创建）
├── serial-x86_64.sock     # 串口socket
└── monitor-x86_64.sock    # QEMU monitor socket
```

## 工作原理

1. **QEMU配置**：修改了`tools/run-qemu.sh`，在nographic模式下使用unix socket替代stdio
2. **多客户端支持**：串口socket支持多个客户端同时连接（MCP服务器 + 开发者终端）
3. **输出分流**：所有输出同时写入socket和日志文件`serial_opt.txt`
4. **命令发送**：通过QEMU monitor socket发送命令（如sendkey模拟键盘输入）

## 权限配置

### QEMU需要sudo权限

DragonOS的QEMU启动脚本需要sudo权限来：
1. 删除共享内存文件（`/dev/shm/dragonos-qemu-shm.ram`）
2. 运行QEMU（某些系统配置需要）

### 配置sudo免密（推荐）

为了MCP服务器能够自动启动QEMU，建议配置sudo免密：

1. **编辑sudoers文件**：
   ```bash
   sudo visudo
   ```

2. **添加以下行**（替换`your_username`为你的用户名）：
   ```
   your_username ALL=(ALL) NOPASSWD: /usr/bin/qemu-system-*, /usr/bin/rm
   ```

3. **或者更精确的配置**（更安全）：
   ```
   your_username ALL=(ALL) NOPASSWD: /usr/bin/qemu-system-x86_64, /usr/bin/qemu-system-riscv64, /usr/bin/qemu-system-loongarch64, /usr/bin/rm -rf /dev/shm/dragonos-qemu-shm.ram
   ```

4. **保存并退出**

5. **验证配置**：
   ```bash
   sudo -n true
   ```
   如果无密码执行成功，说明配置正确。

### 替代方案

如果不想配置sudo免密，可以：

1. **手动启动虚拟机**：在终端中运行 `make run-nographic` 并输入密码
2. **然后使用MCP服务器**：MCP服务器可以连接到已运行的虚拟机

### 检查权限

MCP服务器会自动检查权限配置。调用 `dragonos_vm_status` 工具可以查看权限检查结果。

## 故障排除

### 无法连接到socket

- 确保DragonOS虚拟机正在运行：`make run-nographic`
- 检查socket文件是否存在：`ls -l bin/tmp/hypervisor/`
- 检查socket权限

### 权限错误

- **错误**：`需要sudo权限但未配置免密`
- **解决**：按照上面的"配置sudo免密"部分进行配置
- **或者**：手动运行 `make run-nographic` 一次，然后使用MCP服务器连接

### MCP服务器无法启动

- 检查Python版本：需要Python 3.8+
- 检查依赖是否安装：`pip list | grep mcp`
- 检查PYTHONPATH环境变量

### GDB连接失败

- 确保QEMU使用`-s`参数启动（在`run-qemu.sh`中已配置）
- 检查端口1234是否被占用
- 确保GDB服务器已启动

## 开发

### 添加新工具

在`src/dragonos_mcp/mcp/tools.py`中：

1. 在`list_tools()`中添加工具定义
2. 在`call_tool()`中添加工具处理逻辑

### 测试

```bash
# 运行基础测试
python -m pytest tests/

# 手动测试MCP服务器
python -m dragonos_mcp.server
```

## 许可证

与DragonOS项目相同。

